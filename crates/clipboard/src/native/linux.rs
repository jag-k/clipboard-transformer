use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::os::fd::AsRawFd;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask,
    PropMode, Property, PropertyNotifyEvent, SelectionNotifyEvent, SelectionRequestEvent,
    WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::{
    decode_latin1, decode_mime_text, ClipboardFormat, ClipboardItem, ClipboardPlatform,
    NativeRepresentation,
};
use crate::{format_requests_native_kind, ClipboardBackend, ClipboardMetadata};

pub(super) const SOURCE_MARKER_MIME: &str = "application/x-clipboard-transformer";
pub(super) const KDE_PASSWORD_HINT_MIME: &str = "x-kde-passwordManagerHint";
pub(super) const SELECTION_TIMEOUT: Duration = Duration::from_secs(5);
const PROPERTY_CHUNK_LONGS: u32 = 16 * 1024;
const HYGIENE_MARKER_MAX_BYTES: u64 = 64;

enum SelectionRead {
    Unavailable,
    TooLarge,
    Data(SelectionData),
}

struct SelectionData {
    bytes: Vec<u8>,
    returned_type: Atom,
    unit_bits: u8,
    incremental: bool,
}

struct OutgoingTransfer {
    property_type: Atom,
    unit_bits: u8,
    bytes: Vec<u8>,
    offset: usize,
    chunk_size: usize,
}

struct X11Offer {
    property_type: Atom,
    unit_bits: u8,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinuxClipboardBackendKind {
    X11Xwayland,
    WaylandExtDataControl,
    WaylandWlrDataControl,
}

impl fmt::Display for LinuxClipboardBackendKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::X11Xwayland => formatter.write_str("x11-xwayland"),
            Self::WaylandExtDataControl => formatter.write_str("wayland-ext-data-control"),
            Self::WaylandWlrDataControl => formatter.write_str("wayland-wlr-data-control"),
        }
    }
}

pub fn probe_clipboard_backend() -> Result<Option<LinuxClipboardBackendKind>> {
    match super::wayland::probe() {
        Ok(Some(kind)) => return Ok(Some(kind)),
        Ok(None) => {}
        Err(error) => {
            log::info!("Linux Wayland clipboard probe failed: {error:#}");
        }
    }
    if std::env::var_os("DISPLAY").is_some() {
        match X11ClipboardBackend::probe() {
            Ok(true) => return Ok(Some(LinuxClipboardBackendKind::X11Xwayland)),
            Ok(false) => {}
            Err(error) => {
                log::info!("Linux X11 clipboard probe failed: {error:#}");
            }
        }
    }
    Ok(None)
}

pub enum LinuxClipboardBackend {
    X11(Box<X11ClipboardBackend>),
    Wayland(super::wayland::WaylandClipboardBackend),
}

impl LinuxClipboardBackend {
    pub fn new() -> Result<Self> {
        match probe_clipboard_backend()? {
            Some(
                LinuxClipboardBackendKind::WaylandExtDataControl
                | LinuxClipboardBackendKind::WaylandWlrDataControl,
            ) => Ok(Self::Wayland(
                super::wayland::WaylandClipboardBackend::new()?,
            )),
            Some(LinuxClipboardBackendKind::X11Xwayland) => {
                Ok(Self::X11(Box::new(X11ClipboardBackend::new()?)))
            }
            // Reports the technical condition only. The setup link belongs to
            // whichever host presents this, which is why it is not named here:
            // this crate must not depend on the application.
            None => anyhow::bail!(
                "clipboard observation is unavailable: no supported X11/XWayland or Wayland data-control backend (session_type={}, DISPLAY={}, WAYLAND_DISPLAY={})",
                std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".into()),
                std::env::var("DISPLAY").unwrap_or_else(|_| "unset".into()),
                std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "unset".into()),
            ),
        }
    }
}

impl ClipboardBackend for LinuxClipboardBackend {
    fn change_count(&mut self) -> Result<Option<u64>> {
        match self {
            Self::X11(backend) => backend.change_count(),
            Self::Wayland(backend) => backend.change_count(),
        }
    }

    fn read(&mut self) -> Result<Option<ClipboardItem>> {
        match self {
            Self::X11(backend) => backend.read(),
            Self::Wayland(backend) => backend.read(),
        }
    }

    fn metadata(&mut self) -> Result<ClipboardMetadata> {
        match self {
            Self::X11(backend) => backend.metadata(),
            Self::Wayland(backend) => backend.metadata(),
        }
    }

    fn read_limited(&mut self, max_bytes: u64) -> Result<Option<ClipboardItem>> {
        match self {
            Self::X11(backend) => backend.read_limited(max_bytes),
            Self::Wayland(backend) => backend.read_limited(max_bytes),
        }
    }

    fn read_formats_limited(
        &mut self,
        formats: &BTreeSet<ClipboardFormat>,
        max_bytes: u64,
    ) -> Result<Option<ClipboardItem>> {
        match self {
            Self::X11(backend) => backend.read_formats_limited(formats, max_bytes),
            Self::Wayland(backend) => backend.read_formats_limited(formats, max_bytes),
        }
    }

    fn write(&mut self, content: &ClipboardItem) -> Result<()> {
        match self {
            Self::X11(backend) => backend.write(content),
            Self::Wayland(backend) => backend.write(content),
        }
    }
}

pub struct X11ClipboardBackend {
    connection: RustConnection,
    window: u32,
    clipboard: Atom,
    targets: Atom,
    property: Atom,
    incr: Atom,
    marker: Atom,
    kde_password_hint: Atom,
    sequence: u64,
    offers: BTreeMap<Atom, X11Offer>,
    outgoing: BTreeMap<(u32, Atom), OutgoingTransfer>,
}

impl X11ClipboardBackend {
    pub fn probe() -> Result<bool> {
        let (connection, _) = x11rb::connect(None).context("connect to X11 display")?;
        let extension = connection
            .extension_information(x11rb::protocol::xfixes::X11_EXTENSION_NAME)?
            .is_some();
        if extension {
            connection.xfixes_query_version(5, 0)?.reply()?;
        }
        Ok(extension)
    }

    pub fn new() -> Result<Self> {
        let (connection, screen_number) =
            x11rb::connect(None).context("connect to X11/XWayland clipboard display")?;
        let screen = &connection.setup().roots[screen_number];
        let window = connection.generate_id()?;
        connection.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            screen.root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;
        let clipboard = intern_atom(&connection, "CLIPBOARD")?;
        let targets = intern_atom(&connection, "TARGETS")?;
        let property = intern_atom(&connection, "CLIPBOARD_TRANSFORMER_SELECTION")?;
        let incr = intern_atom(&connection, "INCR")?;
        let marker = intern_atom(&connection, SOURCE_MARKER_MIME)?;
        let kde_password_hint = intern_atom(&connection, KDE_PASSWORD_HINT_MIME)?;
        connection.xfixes_query_version(5, 0)?.reply()?;
        connection.xfixes_select_selection_input(
            window,
            clipboard,
            SelectionEventMask::SET_SELECTION_OWNER
                | SelectionEventMask::SELECTION_WINDOW_DESTROY
                | SelectionEventMask::SELECTION_CLIENT_CLOSE,
        )?;
        connection.flush()?;
        Ok(Self {
            connection,
            window,
            clipboard,
            targets,
            property,
            incr,
            marker,
            kde_password_hint,
            sequence: 1,
            offers: BTreeMap::new(),
            outgoing: BTreeMap::new(),
        })
    }

    fn drain_change_events(&mut self) -> Result<()> {
        while let Some(event) = self.connection.poll_for_event()? {
            match event {
                x11rb::protocol::Event::XfixesSelectionNotify(event) => {
                    self.sequence = self.sequence.wrapping_add(1).max(1);
                    if event.owner != self.window {
                        self.offers.clear();
                        self.outgoing.clear();
                    }
                }
                x11rb::protocol::Event::SelectionRequest(event) => {
                    self.handle_selection_request(event)?;
                }
                x11rb::protocol::Event::PropertyNotify(event) => {
                    self.advance_outgoing_transfer(event)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn handle_selection_request(&mut self, request: SelectionRequestEvent) -> Result<()> {
        if request.owner != self.window || request.selection != self.clipboard {
            return Ok(());
        }
        let property = if request.property == u32::from(AtomEnum::NONE) {
            request.target
        } else {
            request.property
        };
        let mut delivered_property = u32::from(AtomEnum::NONE);

        if request.target == self.targets {
            let mut targets = self.offers.keys().copied().collect::<Vec<_>>();
            targets.push(self.targets);
            targets.push(self.marker);
            self.connection.change_property32(
                PropMode::REPLACE,
                request.requestor,
                property,
                AtomEnum::ATOM,
                &targets,
            )?;
            delivered_property = property;
        } else if request.target == self.marker {
            self.connection.change_property8(
                PropMode::REPLACE,
                request.requestor,
                property,
                self.marker,
                &[1],
            )?;
            delivered_property = property;
        } else if let Some(offer) = self.offers.get(&request.target) {
            let safe_request_bytes = self.connection.maximum_request_bytes().saturating_sub(1024);
            if offer.bytes.len() <= safe_request_bytes {
                change_property_bytes(
                    &self.connection,
                    request.requestor,
                    property,
                    offer.property_type,
                    offer.unit_bits,
                    &offer.bytes,
                )?;
                delivered_property = property;
            } else {
                self.connection.change_window_attributes(
                    request.requestor,
                    &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
                )?;
                let length = u32::try_from(offer.bytes.len()).unwrap_or(u32::MAX);
                self.connection.change_property32(
                    PropMode::REPLACE,
                    request.requestor,
                    property,
                    self.incr,
                    &[length],
                )?;
                self.outgoing.insert(
                    (request.requestor, property),
                    OutgoingTransfer {
                        property_type: offer.property_type,
                        unit_bits: offer.unit_bits,
                        bytes: offer.bytes.clone(),
                        offset: 0,
                        chunk_size: aligned_chunk_size(safe_request_bytes, offer.unit_bits),
                    },
                );
                delivered_property = property;
            }
        }

        self.connection.send_event(
            false,
            request.requestor,
            EventMask::NO_EVENT,
            SelectionNotifyEvent {
                response_type: SELECTION_NOTIFY_EVENT,
                sequence: 0,
                time: request.time,
                requestor: request.requestor,
                selection: request.selection,
                target: request.target,
                property: delivered_property,
            },
        )?;
        self.connection.flush()?;
        Ok(())
    }

    fn advance_outgoing_transfer(&mut self, event: PropertyNotifyEvent) -> Result<()> {
        if event.state != Property::DELETE {
            return Ok(());
        }
        let key = (event.window, event.atom);
        let Some(transfer) = self.outgoing.get_mut(&key) else {
            return Ok(());
        };
        if transfer.offset < transfer.bytes.len() {
            let end = transfer
                .offset
                .saturating_add(transfer.chunk_size)
                .min(transfer.bytes.len());
            change_property_bytes(
                &self.connection,
                event.window,
                event.atom,
                transfer.property_type,
                transfer.unit_bits,
                &transfer.bytes[transfer.offset..end],
            )?;
            transfer.offset = end;
        } else {
            change_property_bytes(
                &self.connection,
                event.window,
                event.atom,
                transfer.property_type,
                transfer.unit_bits,
                &[],
            )?;
            self.outgoing.remove(&key);
            if !self
                .outgoing
                .keys()
                .any(|(requestor, _)| *requestor == event.window)
            {
                self.connection.change_window_attributes(
                    event.window,
                    &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT),
                )?;
            }
        }
        self.connection.flush()?;
        Ok(())
    }

    fn wait_for_event_bounded(&self) -> Result<x11rb::protocol::Event> {
        if let Some(event) = self.connection.poll_for_event()? {
            return Ok(event);
        }
        let mut descriptor = libc::pollfd {
            fd: self.connection.stream().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout = i32::try_from(SELECTION_TIMEOUT.as_millis()).unwrap_or(i32::MAX);
        let ready = unsafe { libc::poll(&mut descriptor, 1, timeout) };
        if ready < 0 {
            return Err(std::io::Error::last_os_error())
                .context("wait for X11 clipboard selection response");
        }
        if ready == 0 {
            anyhow::bail!(
                "timed out after {}s waiting for X11 clipboard selection response",
                SELECTION_TIMEOUT.as_secs()
            );
        }
        self.connection
            .wait_for_event()
            .context("read X11 clipboard selection event")
    }
}

impl ClipboardBackend for X11ClipboardBackend {
    fn change_count(&mut self) -> Result<Option<u64>> {
        self.drain_change_events()?;
        Ok(Some(self.sequence))
    }

    fn read(&mut self) -> Result<Option<ClipboardItem>> {
        self.read_limited(0)
    }

    fn metadata(&mut self) -> Result<ClipboardMetadata> {
        let targets = self.read_targets()?;
        if targets.contains(&self.marker) {
            return Ok(ClipboardMetadata::ignored());
        }
        if targets.contains(&self.kde_password_hint) {
            let marker =
                self.read_selection(self.kde_password_hint, Some(HYGIENE_MARKER_MAX_BYTES))?;
            if matches!(
                marker,
                SelectionRead::Data(SelectionData { bytes, .. })
                    if marker_is_secret(&bytes)
            ) {
                log::info!(
                    "{}",
                    "clipboard item ignored from x-kde-passwordManagerHint=secret",
                );
                return Ok(ClipboardMetadata::ignored());
            }
        }
        Ok(ClipboardMetadata::readable(None))
    }

    fn read_limited(&mut self, max_bytes: u64) -> Result<Option<ClipboardItem>> {
        let targets = self.read_targets()?;
        if targets.is_empty() {
            return Ok(None);
        }
        let mut item = ClipboardItem::for_platform(ClipboardPlatform::X11);
        let mut total = 0_u128;
        for target in targets {
            let name = atom_name(&self.connection, target)?;
            if target == self.marker
                || target == self.kde_password_hint
                || classify_x11_target(&name) != X11TargetClass::Payload
            {
                continue;
            }
            let remaining = if max_bytes == 0 {
                None
            } else {
                Some(max_bytes.saturating_sub(u64::try_from(total).unwrap_or(u64::MAX)))
            };
            let selection = match self.read_selection(target, remaining)? {
                SelectionRead::Unavailable => continue,
                SelectionRead::TooLarge => {
                    log::info!("clipboard item ignored while reading size_bytes>{max_bytes}");
                    return Ok(None);
                }
                SelectionRead::Data(data) => data,
            };
            total = total.saturating_add(selection.bytes.len() as u128);
            let returned_type = atom_name(&self.connection, selection.returned_type)?;
            let representation = NativeRepresentation::x11(
                name.clone(),
                returned_type.clone(),
                selection.unit_bits,
                selection.bytes,
                selection.incremental,
            );
            derive_x11_semantics(&mut item, &representation, &returned_type);
            item.set_native(representation);
        }
        if item.representations().is_empty() {
            Ok(None)
        } else {
            Ok(Some(item))
        }
    }

    fn read_formats_limited(
        &mut self,
        formats: &BTreeSet<ClipboardFormat>,
        max_bytes: u64,
    ) -> Result<Option<ClipboardItem>> {
        let targets = self.read_targets()?;
        let mut item = ClipboardItem::for_platform(ClipboardPlatform::X11);
        let mut total = 0_u128;
        for target in targets {
            let name = atom_name(&self.connection, target)?;
            if target == self.marker
                || target == self.kde_password_hint
                || classify_x11_target(&name) != X11TargetClass::Payload
                || !format_requests_native_kind(formats, &name)
            {
                continue;
            }
            let remaining = (max_bytes > 0)
                .then(|| max_bytes.saturating_sub(u64::try_from(total).unwrap_or(u64::MAX)));
            let selection = match self.read_selection(target, remaining)? {
                SelectionRead::Unavailable => continue,
                SelectionRead::TooLarge => {
                    log::info!("clipboard item ignored while reading size_bytes>{max_bytes}");
                    return Ok(None);
                }
                SelectionRead::Data(data) => data,
            };
            total = total.saturating_add(selection.bytes.len() as u128);
            let returned_type = atom_name(&self.connection, selection.returned_type)?;
            let representation = NativeRepresentation::x11(
                name,
                returned_type.clone(),
                selection.unit_bits,
                selection.bytes,
                selection.incremental,
            );
            derive_x11_semantics(&mut item, &representation, &returned_type);
            item.set_native(representation);
        }
        Ok((!item.representations().is_empty()).then_some(item))
    }

    fn write(&mut self, content: &ClipboardItem) -> Result<()> {
        let mut offers = BTreeMap::new();
        for offer in linux_offers(content) {
            let target = intern_atom(&self.connection, &offer.kind)?;
            let property_type = intern_atom(
                &self.connection,
                offer.returned_type.as_deref().unwrap_or(&offer.kind),
            )?;
            offers.insert(
                target,
                X11Offer {
                    property_type,
                    unit_bits: offer.unit_bits,
                    bytes: offer.bytes,
                },
            );
        }
        if offers.is_empty() {
            anyhow::bail!("clipboard item has no writable Linux representations");
        }

        self.offers = offers;
        self.connection
            .set_selection_owner(self.window, self.clipboard, x11rb::CURRENT_TIME)?;
        self.connection.flush()?;
        let owner = self
            .connection
            .get_selection_owner(self.clipboard)?
            .reply()?
            .owner;
        if owner != self.window {
            anyhow::bail!("failed to claim the X11 CLIPBOARD selection");
        }
        Ok(())
    }
}

fn derive_x11_semantics(
    item: &mut ClipboardItem,
    representation: &NativeRepresentation,
    returned_type: &str,
) {
    let kind = representation.kind();
    let bytes = representation.data();
    let source = vec![kind.to_string()];
    let lower = kind.to_ascii_lowercase();
    let base = lower
        .split_once(';')
        .map_or(lower.as_str(), |(base, _)| base)
        .trim();
    if matches!(kind, "UTF8_STRING" | "TEXT")
        || returned_type == "UTF8_STRING"
        || base == "text/plain"
    {
        let decoded = if base == "text/plain" {
            decode_mime_text(kind, bytes)
        } else {
            std::str::from_utf8(bytes).ok().map(str::to_owned)
        };
        if let Some(text) = decoded {
            item.set_derived_text(text.trim_end_matches('\0'), source);
        }
    } else if kind == "STRING" || returned_type == "STRING" {
        item.set_derived_text(decode_latin1(bytes).trim_end_matches('\0'), source);
    } else if (kind == "COMPOUND_TEXT" || returned_type == "COMPOUND_TEXT") && bytes.is_ascii() {
        // ASCII is a safe subset of COMPOUND_TEXT. Non-ASCII COMPOUND_TEXT is
        // retained raw until a standards-compliant converter is available.
        item.set_derived_text(
            std::str::from_utf8(bytes)
                .expect("ASCII is UTF-8")
                .trim_end_matches('\0'),
            source,
        );
    } else if base == "text/html" {
        if let Some(html) = decode_mime_text(kind, bytes) {
            item.set_derived_html(html, source);
        }
    } else if base == "text/uri-list" {
        if let Some(url) = decode_mime_text(kind, bytes) {
            let url = url.trim();
            item.set_derived_url(url, source.clone());
            if url.starts_with("file:") {
                item.set_derived_file_url(url, source);
            }
        }
    } else if matches!(base, "text/rtf" | "application/rtf") {
        if let Some(rtf) = decode_mime_text(kind, bytes) {
            item.set_derived_rtf(rtf, source);
        }
    }
}

impl X11ClipboardBackend {
    fn read_targets(&mut self) -> Result<Vec<Atom>> {
        let data = match self.read_selection(self.targets, Some(1024 * 1024))? {
            SelectionRead::Data(data) => data,
            SelectionRead::Unavailable | SelectionRead::TooLarge => return Ok(Vec::new()),
        };
        if data.unit_bits != 32 {
            return Ok(Vec::new());
        }
        Ok(data
            .bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_ne_bytes(chunk.try_into().expect("four-byte atom")))
            .collect())
    }

    fn read_selection(&mut self, target: Atom, max_bytes: Option<u64>) -> Result<SelectionRead> {
        self.connection
            .delete_property(self.window, self.property)?;
        self.connection.convert_selection(
            self.window,
            self.clipboard,
            target,
            self.property,
            x11rb::CURRENT_TIME,
        )?;
        self.connection.flush()?;

        loop {
            match self.wait_for_event_bounded()? {
                x11rb::protocol::Event::SelectionNotify(event) => {
                    if event.requestor != self.window || event.selection != self.clipboard {
                        continue;
                    }
                    if event.property == u32::from(AtomEnum::NONE) {
                        return Ok(SelectionRead::Unavailable);
                    }
                    let first = self
                        .connection
                        .get_property(
                            false,
                            self.window,
                            self.property,
                            AtomEnum::ANY,
                            0,
                            PROPERTY_CHUNK_LONGS,
                        )?
                        .reply()?;
                    if first.type_ == self.incr {
                        self.connection
                            .delete_property(self.window, self.property)?;
                        self.connection.flush()?;
                        return self.read_incremental_selection(max_bytes);
                    }
                    return self.read_normal_property(first, max_bytes, false);
                }
                x11rb::protocol::Event::XfixesSelectionNotify(_) => {
                    self.sequence = self.sequence.wrapping_add(1).max(1);
                }
                x11rb::protocol::Event::SelectionRequest(event) => {
                    self.handle_selection_request(event)?;
                }
                x11rb::protocol::Event::PropertyNotify(event) => {
                    self.advance_outgoing_transfer(event)?;
                }
                _ => {}
            }
        }
    }

    fn read_normal_property(
        &self,
        mut property: x11rb::protocol::xproto::GetPropertyReply,
        max_bytes: Option<u64>,
        incremental: bool,
    ) -> Result<SelectionRead> {
        if !matches!(property.format, 8 | 16 | 32) || property.type_ == u32::from(AtomEnum::NONE) {
            return Ok(SelectionRead::Unavailable);
        }
        let returned_type = property.type_;
        let unit_bits = property.format;
        let mut bytes = Vec::new();
        let mut offset = 0_u32;
        loop {
            if exceeds_limit(bytes.len(), property.value.len(), max_bytes) {
                self.connection
                    .delete_property(self.window, self.property)?;
                self.connection.flush()?;
                return Ok(SelectionRead::TooLarge);
            }
            bytes.extend_from_slice(&property.value);
            if property.bytes_after == 0 {
                self.connection
                    .delete_property(self.window, self.property)?;
                self.connection.flush()?;
                return Ok(SelectionRead::Data(SelectionData {
                    bytes,
                    returned_type,
                    unit_bits,
                    incremental,
                }));
            }
            offset = offset.saturating_add(
                u32::try_from(property.value.len().div_ceil(4)).unwrap_or(u32::MAX),
            );
            property = self
                .connection
                .get_property(
                    false,
                    self.window,
                    self.property,
                    AtomEnum::ANY,
                    offset,
                    PROPERTY_CHUNK_LONGS,
                )?
                .reply()?;
            if property.type_ != returned_type || property.format != unit_bits {
                self.connection
                    .delete_property(self.window, self.property)?;
                self.connection.flush()?;
                anyhow::bail!("X11 selection property changed type or format during one transfer");
            }
        }
    }

    fn read_incremental_selection(&mut self, max_bytes: Option<u64>) -> Result<SelectionRead> {
        let mut bytes = Vec::new();
        let mut too_large = false;
        let mut descriptor = None;
        loop {
            match self.wait_for_event_bounded()? {
                x11rb::protocol::Event::PropertyNotify(event)
                    if event.window == self.window
                        && event.atom == self.property
                        && event.state == Property::NEW_VALUE =>
                {
                    let first = self
                        .connection
                        .get_property(
                            false,
                            self.window,
                            self.property,
                            AtomEnum::ANY,
                            0,
                            PROPERTY_CHUNK_LONGS,
                        )?
                        .reply()?;
                    if first.value.is_empty() && first.bytes_after == 0 {
                        self.connection
                            .delete_property(self.window, self.property)?;
                        self.connection.flush()?;
                        return Ok(if too_large {
                            SelectionRead::TooLarge
                        } else {
                            let Some((returned_type, unit_bits)) = descriptor else {
                                return Ok(SelectionRead::Unavailable);
                            };
                            SelectionRead::Data(SelectionData {
                                bytes,
                                returned_type,
                                unit_bits,
                                incremental: true,
                            })
                        });
                    }
                    let remaining = if too_large {
                        Some(0)
                    } else {
                        max_bytes.map(|max_bytes| {
                            max_bytes.saturating_sub(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
                        })
                    };
                    match self.read_normal_property(first, remaining, true)? {
                        SelectionRead::Data(chunk) if !too_large => {
                            if let Some((returned_type, unit_bits)) = descriptor {
                                if returned_type != chunk.returned_type
                                    || unit_bits != chunk.unit_bits
                                {
                                    anyhow::bail!(
                                        "X11 INCR chunks changed property type or format"
                                    );
                                }
                            } else {
                                descriptor = Some((chunk.returned_type, chunk.unit_bits));
                            }
                            bytes.extend_from_slice(&chunk.bytes);
                        }
                        SelectionRead::TooLarge => {
                            too_large = true;
                            bytes.clear();
                        }
                        SelectionRead::Unavailable | SelectionRead::Data(_) => {}
                    }
                }
                x11rb::protocol::Event::XfixesSelectionNotify(_) => {
                    self.sequence = self.sequence.wrapping_add(1).max(1);
                }
                x11rb::protocol::Event::SelectionRequest(event) => {
                    self.handle_selection_request(event)?;
                }
                x11rb::protocol::Event::PropertyNotify(event) => {
                    self.advance_outgoing_transfer(event)?;
                }
                _ => {}
            }
        }
    }
}

impl Drop for X11ClipboardBackend {
    fn drop(&mut self) {
        let _ = self.connection.destroy_window(self.window);
        let _ = self.connection.flush();
    }
}

pub(super) struct LinuxOffer {
    pub kind: String,
    pub returned_type: Option<String>,
    pub unit_bits: u8,
    pub bytes: Vec<u8>,
}

pub(super) fn linux_offers(content: &ClipboardItem) -> Vec<LinuxOffer> {
    let mut offers = Vec::new();
    if matches!(
        content.platform(),
        ClipboardPlatform::Wayland | ClipboardPlatform::X11
    ) {
        for representation in content.representations() {
            if stale_for_authored_semantic(content, representation.kind()) {
                continue;
            }
            offers.push(LinuxOffer {
                kind: representation.kind().to_string(),
                returned_type: representation.returned_type().map(str::to_string),
                unit_bits: representation.unit_bits().unwrap_or(8),
                bytes: representation.data().to_vec(),
            });
        }
    }

    if let Some(text) = content
        .text_semantic()
        .filter(|semantic| semantic.is_authored())
        .map(|semantic| semantic.value().as_bytes().to_vec())
    {
        for kind in ["UTF8_STRING", "text/plain", "text/plain;charset=utf-8"] {
            upsert_linux_offer(&mut offers, kind, text.clone());
        }
    }
    if let Some(html) = content
        .html_semantic()
        .filter(|semantic| semantic.is_authored())
    {
        upsert_linux_offer(&mut offers, "text/html", html.value().as_bytes().to_vec());
    }
    if let Some(rtf) = content
        .rtf_semantic()
        .filter(|semantic| semantic.is_authored())
    {
        upsert_linux_offer(&mut offers, "text/rtf", rtf.value().as_bytes().to_vec());
    }
    if let Some(url) = content
        .url_semantic()
        .filter(|semantic| semantic.is_authored())
        .or_else(|| {
            content
                .file_url_semantic()
                .filter(|semantic| semantic.is_authored())
        })
    {
        upsert_linux_offer(
            &mut offers,
            "text/uri-list",
            url.value().as_bytes().to_vec(),
        );
    }
    offers
}

fn upsert_linux_offer(offers: &mut Vec<LinuxOffer>, kind: &str, bytes: Vec<u8>) {
    if let Some(offer) = offers.iter_mut().find(|offer| offer.kind == kind) {
        offer.returned_type = None;
        offer.unit_bits = 8;
        offer.bytes = bytes;
    } else {
        offers.push(LinuxOffer {
            kind: kind.into(),
            returned_type: None,
            unit_bits: 8,
            bytes,
        });
    }
}

fn stale_for_authored_semantic(content: &ClipboardItem, kind: &str) -> bool {
    let lower = kind.to_ascii_lowercase();
    if matches!(kind, "UTF8_STRING" | "STRING" | "TEXT" | "COMPOUND_TEXT")
        || lower.starts_with("text/plain")
    {
        return content
            .text_semantic()
            .is_some_and(|semantic| semantic.is_authored());
    }
    match lower.as_str() {
        "text/html" => content
            .html_semantic()
            .is_some_and(|semantic| semantic.is_authored()),
        "text/rtf" | "application/rtf" => content
            .rtf_semantic()
            .is_some_and(|semantic| semantic.is_authored()),
        "text/uri-list" => content
            .url_semantic()
            .or_else(|| content.file_url_semantic())
            .is_some_and(|semantic| semantic.is_authored()),
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum X11TargetClass {
    Payload,
    Metadata,
    ControlOrSideEffect,
}

fn classify_x11_target(name: &str) -> X11TargetClass {
    match name {
        "TARGETS" | "TIMESTAMP" | "LENGTH" | "LIST_LENGTH" | "TARGET_SIZES" | "HOST_NAME"
        | "USER" | "CLASS" | "NAME" | "CLIENT_WINDOW" | "OWNER_OS" => X11TargetClass::Metadata,
        "DELETE" | "INSERT_SELECTION" | "INSERT_PROPERTY" | "MULTIPLE" | "SAVE_TARGETS"
        | "INCR" | "CLIPBOARD_MANAGER" => X11TargetClass::ControlOrSideEffect,
        _ => X11TargetClass::Payload,
    }
}

fn marker_is_secret(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes)
        .ok()
        .is_some_and(|value| value.trim() == "secret")
}

fn aligned_chunk_size(bytes: usize, unit_bits: u8) -> usize {
    let unit_bytes = usize::from(unit_bits / 8).max(1);
    bytes.saturating_sub(bytes % unit_bytes).max(unit_bytes)
}

fn change_property_bytes(
    connection: &RustConnection,
    window: u32,
    property: Atom,
    property_type: Atom,
    unit_bits: u8,
    bytes: &[u8],
) -> Result<()> {
    match unit_bits {
        8 => connection.change_property8(
            PropMode::REPLACE,
            window,
            property,
            property_type,
            bytes,
        )?,
        16 if bytes.len().is_multiple_of(2) => {
            let values = bytes
                .chunks_exact(2)
                .map(|chunk| u16::from_ne_bytes([chunk[0], chunk[1]]))
                .collect::<Vec<_>>();
            connection.change_property16(
                PropMode::REPLACE,
                window,
                property,
                property_type,
                &values,
            )?
        }
        32 if bytes.len().is_multiple_of(4) => {
            let values = bytes
                .chunks_exact(4)
                .map(|chunk| u32::from_ne_bytes(chunk.try_into().expect("four bytes")))
                .collect::<Vec<_>>();
            connection.change_property32(
                PropMode::REPLACE,
                window,
                property,
                property_type,
                &values,
            )?
        }
        16 | 32 => anyhow::bail!(
            "X11 {unit_bits}-bit property has misaligned {}-byte payload",
            bytes.len()
        ),
        other => anyhow::bail!("invalid X11 property format {other}; expected 8, 16, or 32"),
    };
    Ok(())
}

fn intern_atom(connection: &RustConnection, name: &str) -> Result<Atom> {
    let bytes = name
        .chars()
        .map(|character| u8::try_from(u32::from(character)))
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap_or_else(|_| name.as_bytes().to_vec());
    Ok(connection.intern_atom(false, &bytes)?.reply()?.atom)
}

fn atom_name(connection: &RustConnection, atom: Atom) -> Result<String> {
    let reply = connection.get_atom_name(atom)?.reply()?;
    Ok(decode_latin1(&reply.name))
}

fn exceeds_limit(existing: usize, additional: usize, max_bytes: Option<u64>) -> bool {
    max_bytes.is_some_and(|max_bytes| {
        u128::try_from(existing)
            .unwrap_or(u128::MAX)
            .saturating_add(u128::try_from(additional).unwrap_or(u128::MAX))
            > u128::from(max_bytes)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_formats_map_to_linux_mime_offers() {
        let mut item = ClipboardItem::from_text("cat");
        item.set_html("<b>cat</b>");
        item.set_rtf(r"{\rtf1 cat}");
        item.set_url("https://example.test/cat");

        let offers = linux_offers(&item)
            .into_iter()
            .map(|offer| (offer.kind, offer.bytes))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(offers.get("UTF8_STRING"), Some(&b"cat".to_vec()));
        assert_eq!(offers.get("text/plain"), Some(&b"cat".to_vec()));
        assert_eq!(
            offers.get("text/plain;charset=utf-8"),
            Some(&b"cat".to_vec())
        );
        assert_eq!(offers.get("text/html"), Some(&b"<b>cat</b>".to_vec()));
        assert_eq!(offers.get("text/rtf"), Some(&br"{\rtf1 cat}".to_vec()));
        assert_eq!(
            offers.get("text/uri-list"),
            Some(&b"https://example.test/cat".to_vec())
        );
    }

    #[test]
    fn opaque_linux_mime_round_trips_to_an_offer() {
        let mut item = ClipboardItem::for_platform(ClipboardPlatform::Wayland);
        item.set_native(NativeRepresentation::named("image/png", vec![1, 2, 3]));

        assert_eq!(
            linux_offers(&item)
                .into_iter()
                .map(|offer| (offer.kind, offer.bytes))
                .collect::<Vec<_>>(),
            vec![("image/png".to_string(), vec![1, 2, 3])]
        );
    }

    #[test]
    fn x11_control_and_metadata_targets_are_not_payloads() {
        for target in [
            "DELETE",
            "INSERT_SELECTION",
            "INSERT_PROPERTY",
            "MULTIPLE",
            "SAVE_TARGETS",
            "TIMESTAMP",
            "TARGETS",
            "LENGTH",
            "LIST_LENGTH",
            "TARGET_SIZES",
            "INCR",
        ] {
            assert_ne!(classify_x11_target(target), X11TargetClass::Payload);
        }
        for target in ["UTF8_STRING", "STRING", "text/html", "image/png"] {
            assert_eq!(classify_x11_target(target), X11TargetClass::Payload);
        }
    }

    #[test]
    fn kde_password_hint_requires_exact_trimmed_secret_value() {
        assert!(marker_is_secret(b" secret\n"));
        assert!(!marker_is_secret(b"secret-value"));
        assert!(!marker_is_secret(b"SECRET"));
        assert!(!marker_is_secret(&[0xff]));
    }
}
