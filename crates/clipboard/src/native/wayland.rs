use std::collections::BTreeSet;
use std::io::{ErrorKind, Read};
use std::os::fd::AsRawFd;
use std::time::Instant;

use anyhow::{Context, Result};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::{event_created_child, Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::ext::data_control::v1::client as ext;
use wayland_protocols_wlr::data_control::v1::client as wlr;
use wl_clipboard_rs::{copy, paste};

use crate::{
    decode_latin1, decode_mime_text, ClipboardFormat, ClipboardItem, ClipboardPlatform,
    NativeRepresentation,
};
use crate::{format_requests_native_kind, ClipboardBackend, ClipboardMetadata};

use super::linux::{
    linux_offers, LinuxClipboardBackendKind, KDE_PASSWORD_HINT_MIME, SELECTION_TIMEOUT,
    SOURCE_MARKER_MIME,
};

const HYGIENE_MARKER_MAX_BYTES: u64 = 64;

use ext::ext_data_control_device_v1::ExtDataControlDeviceV1;
use ext::ext_data_control_manager_v1::ExtDataControlManagerV1;
use ext::ext_data_control_offer_v1::ExtDataControlOfferV1;
use wlr::zwlr_data_control_device_v1::ZwlrDataControlDeviceV1;
use wlr::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1;
use wlr::zwlr_data_control_offer_v1::ZwlrDataControlOfferV1;

enum Manager {
    Ext(ExtDataControlManagerV1),
    Wlr(ZwlrDataControlManagerV1),
}

enum Device {
    Ext(ExtDataControlDeviceV1),
    Wlr(ZwlrDataControlDeviceV1),
}

impl Manager {
    fn device(&self, seat: &WlSeat, queue: &QueueHandle<WatcherState>) -> Device {
        match self {
            Self::Ext(manager) => Device::Ext(manager.get_data_device(seat, queue, ())),
            Self::Wlr(manager) => Device::Wlr(manager.get_data_device(seat, queue, ())),
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        match self {
            Self::Ext(device) => device.destroy(),
            Self::Wlr(device) => device.destroy(),
        }
    }
}

struct RegistryState;

impl Dispatch<WlRegistry, GlobalListContents> for RegistryState {
    fn event(
        _state: &mut Self,
        _proxy: &WlRegistry,
        _event: <WlRegistry as Proxy>::Event,
        _data: &GlobalListContents,
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
    }
}

pub fn probe() -> Result<Option<LinuxClipboardBackendKind>> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Ok(None);
    }
    let connection = Connection::connect_to_env().context("connect to Wayland compositor")?;
    let (globals, _queue) =
        registry_queue_init::<RegistryState>(&connection).context("discover Wayland globals")?;
    Ok(globals.contents().with_list(|globals| {
        if has_global(globals, ExtDataControlManagerV1::interface().name) {
            Some(LinuxClipboardBackendKind::WaylandExtDataControl)
        } else if has_global(globals, ZwlrDataControlManagerV1::interface().name) {
            Some(LinuxClipboardBackendKind::WaylandWlrDataControl)
        } else {
            None
        }
    }))
}

fn has_global(globals: &[wayland_client::globals::Global], interface: &str) -> bool {
    globals
        .iter()
        .any(|global| global.interface == interface && global.version >= 1)
}

struct WatcherState {
    sequence: u64,
    _seats: Vec<WlSeat>,
    _devices: Vec<Device>,
    _manager: Manager,
}

impl Dispatch<WlRegistry, GlobalListContents> for WatcherState {
    fn event(
        _state: &mut Self,
        _proxy: &WlRegistry,
        _event: <WlRegistry as Proxy>::Event,
        _data: &GlobalListContents,
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for WatcherState {
    fn event(
        _state: &mut Self,
        _proxy: &WlSeat,
        _event: wl_seat::Event,
        _data: &(),
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
    }
}

macro_rules! empty_manager_dispatch {
    ($interface:ty) => {
        impl Dispatch<$interface, ()> for WatcherState {
            fn event(
                _state: &mut Self,
                _proxy: &$interface,
                _event: <$interface as Proxy>::Event,
                _data: &(),
                _connection: &Connection,
                _queue: &QueueHandle<Self>,
            ) {
            }
        }
    };
}

empty_manager_dispatch!(ExtDataControlManagerV1);
empty_manager_dispatch!(ZwlrDataControlManagerV1);

macro_rules! device_dispatch {
    ($interface:ty, $offer:ty, $opcode:path, $selection:path, $finished:path) => {
        impl Dispatch<$interface, ()> for WatcherState {
            fn event(
                state: &mut Self,
                _proxy: &$interface,
                event: <$interface as Proxy>::Event,
                _data: &(),
                _connection: &Connection,
                _queue: &QueueHandle<Self>,
            ) {
                match event {
                    $selection { .. } => {
                        state.sequence = state.sequence.wrapping_add(1).max(1);
                    }
                    $finished => {
                        log::info!("{}",
                            "Wayland data-control device was stopped by the compositor",
                        );
                    }
                    _ => {}
                }
            }

            event_created_child!(WatcherState, $interface, [
                $opcode => ($offer, ()),
            ]);
        }
    };
}

device_dispatch!(
    ExtDataControlDeviceV1,
    ExtDataControlOfferV1,
    ext::ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE,
    ext::ext_data_control_device_v1::Event::Selection,
    ext::ext_data_control_device_v1::Event::Finished
);
device_dispatch!(
    ZwlrDataControlDeviceV1,
    ZwlrDataControlOfferV1,
    wlr::zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE,
    wlr::zwlr_data_control_device_v1::Event::Selection,
    wlr::zwlr_data_control_device_v1::Event::Finished
);

macro_rules! offer_dispatch {
    ($interface:ty) => {
        impl Dispatch<$interface, ()> for WatcherState {
            fn event(
                _state: &mut Self,
                _proxy: &$interface,
                _event: <$interface as Proxy>::Event,
                _data: &(),
                _connection: &Connection,
                _queue: &QueueHandle<Self>,
            ) {
            }
        }
    };
}

offer_dispatch!(ExtDataControlOfferV1);
offer_dispatch!(ZwlrDataControlOfferV1);

pub struct WaylandClipboardBackend {
    queue: EventQueue<WatcherState>,
    state: WatcherState,
    kind: LinuxClipboardBackendKind,
}

impl WaylandClipboardBackend {
    pub fn new() -> Result<Self> {
        let connection =
            Connection::connect_to_env().context("connect to Wayland clipboard compositor")?;
        let (globals, mut queue) =
            registry_queue_init::<WatcherState>(&connection).context("discover Wayland globals")?;
        let queue_handle = queue.handle();

        let ext_manager = globals
            .bind::<ExtDataControlManagerV1, _, _>(&queue_handle, 1..=1, ())
            .ok()
            .map(Manager::Ext);
        let (manager, kind) = if let Some(manager) = ext_manager {
            (manager, LinuxClipboardBackendKind::WaylandExtDataControl)
        } else {
            let manager = globals
                .bind::<ZwlrDataControlManagerV1, _, _>(&queue_handle, 1..=1, ())
                .context("Wayland compositor has no ext-data-control or wlr-data-control")?;
            (
                Manager::Wlr(manager),
                LinuxClipboardBackendKind::WaylandWlrDataControl,
            )
        };

        let registry = globals.registry();
        let seats = globals.contents().with_list(|globals| {
            globals
                .iter()
                .filter(|global| {
                    global.interface == WlSeat::interface().name && global.version >= 2
                })
                .map(|global| registry.bind(global.name, 2, &queue_handle, ()))
                .collect::<Vec<_>>()
        });
        if seats.is_empty() {
            anyhow::bail!("Wayland compositor exposed no usable wl_seat");
        }
        let devices = seats
            .iter()
            .map(|seat| manager.device(seat, &queue_handle))
            .collect();
        let mut state = WatcherState {
            sequence: 1,
            _seats: seats,
            _devices: devices,
            _manager: manager,
        };
        queue
            .roundtrip(&mut state)
            .context("initialize Wayland data-control devices")?;

        Ok(Self { queue, state, kind })
    }

    pub fn kind(&self) -> LinuxClipboardBackendKind {
        self.kind
    }

    fn mime_types(&self) -> Result<Vec<String>> {
        match paste::get_mime_types_ordered(paste::ClipboardType::Regular, paste::Seat::Unspecified)
        {
            Ok(mime_types) => Ok(mime_types),
            Err(
                paste::Error::ClipboardEmpty | paste::Error::NoMimeType | paste::Error::NoSeats,
            ) => Ok(Vec::new()),
            Err(error) => Err(error.into()),
        }
    }
}

impl ClipboardBackend for WaylandClipboardBackend {
    fn change_count(&mut self) -> Result<Option<u64>> {
        self.queue
            .roundtrip(&mut self.state)
            .context("poll Wayland clipboard selection")?;
        Ok(Some(self.state.sequence))
    }

    fn read(&mut self) -> Result<Option<ClipboardItem>> {
        self.read_limited(0)
    }

    fn metadata(&mut self) -> Result<ClipboardMetadata> {
        let mime_types = self.mime_types()?;
        if mime_types.iter().any(|mime| mime == SOURCE_MARKER_MIME) {
            return Ok(ClipboardMetadata::ignored());
        }
        if mime_types.iter().any(|mime| mime == KDE_PASSWORD_HINT_MIME)
            && self
                .read_mime_bounded(KDE_PASSWORD_HINT_MIME, Some(HYGIENE_MARKER_MAX_BYTES))?
                .as_deref()
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .is_some_and(|value| value.trim() == "secret")
        {
            log::info!("clipboard item ignored from x-kde-passwordManagerHint=secret");
            return Ok(ClipboardMetadata::ignored());
        }
        Ok(ClipboardMetadata::readable(None))
    }

    fn read_limited(&mut self, max_bytes: u64) -> Result<Option<ClipboardItem>> {
        self.read_selected(None, max_bytes)
    }

    fn read_formats_limited(
        &mut self,
        formats: &BTreeSet<ClipboardFormat>,
        max_bytes: u64,
    ) -> Result<Option<ClipboardItem>> {
        self.read_selected(Some(formats), max_bytes)
    }

    fn write(&mut self, content: &ClipboardItem) -> Result<()> {
        let mut sources = linux_offers(content)
            .into_iter()
            .map(|offer| copy::MimeSource {
                source: copy::Source::Bytes(offer.bytes.into_boxed_slice()),
                mime_type: copy::MimeType::Specific(offer.kind),
            })
            .collect::<Vec<_>>();
        if sources.is_empty() {
            anyhow::bail!("clipboard item has no writable Linux representations");
        }
        sources.push(copy::MimeSource {
            source: copy::Source::Bytes(vec![1].into_boxed_slice()),
            mime_type: copy::MimeType::Specific(SOURCE_MARKER_MIME.into()),
        });
        let mut options = copy::Options::new();
        options
            .clipboard(copy::ClipboardType::Regular)
            .seat(copy::Seat::All)
            .omit_additional_text_mime_types(true);
        options
            .copy_multi(sources)
            .context("write Wayland data-control clipboard")
    }
}

impl WaylandClipboardBackend {
    fn read_selected(
        &mut self,
        formats: Option<&BTreeSet<ClipboardFormat>>,
        max_bytes: u64,
    ) -> Result<Option<ClipboardItem>> {
        let mime_types = self.mime_types()?;
        let mut item = ClipboardItem::for_platform(ClipboardPlatform::Wayland);
        let mut total = 0_u64;

        for mime in mime_types {
            if mime == SOURCE_MARKER_MIME || mime == KDE_PASSWORD_HINT_MIME {
                continue;
            }
            if formats.is_some_and(|formats| !format_requests_native_kind(formats, &mime)) {
                continue;
            }
            let remaining = if max_bytes == 0 {
                None
            } else {
                Some(max_bytes.saturating_sub(total))
            };
            let Some(bytes) = self.read_mime_bounded(&mime, remaining)? else {
                log::info!("clipboard item ignored while reading size_bytes>{max_bytes}");
                return Ok(None);
            };
            total = total.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
            add_representation(&mut item, &mime, bytes);
        }

        if item.representations().is_empty() {
            Ok(None)
        } else {
            Ok(Some(item))
        }
    }
    fn read_mime_bounded(&self, mime: &str, max_bytes: Option<u64>) -> Result<Option<Vec<u8>>> {
        let result = paste::get_contents(
            paste::ClipboardType::Regular,
            paste::Seat::Unspecified,
            paste::MimeType::Specific(mime),
        );
        let (mut reader, _) = match result {
            Ok(contents) => contents,
            Err(paste::Error::ClipboardEmpty | paste::Error::NoMimeType) => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("request Wayland clipboard MIME {mime}"));
            }
        };
        read_pipe_bounded(&mut reader, max_bytes)
    }
}

fn add_representation(item: &mut ClipboardItem, mime: &str, bytes: Vec<u8>) {
    let source = vec![mime.to_string()];
    let lower = mime.to_ascii_lowercase();
    let base = lower
        .split_once(';')
        .map_or(lower.as_str(), |(base, _)| base)
        .trim();
    if base == "text/plain" || matches!(mime, "UTF8_STRING" | "TEXT") {
        if let Some(text) = decode_mime_text(mime, &bytes) {
            item.set_derived_text(text.trim_end_matches('\0'), source);
        }
    } else if mime == "STRING" {
        item.set_derived_text(decode_latin1(&bytes).trim_end_matches('\0'), source);
    } else if base == "text/html" {
        if let Some(html) = decode_mime_text(mime, &bytes) {
            item.set_derived_html(html, source);
        }
    } else if matches!(base, "text/rtf" | "application/rtf") {
        if let Some(rtf) = decode_mime_text(mime, &bytes) {
            item.set_derived_rtf(rtf, source);
        }
    } else if base == "text/uri-list" {
        if let Some(url) = decode_mime_text(mime, &bytes) {
            let url = url.trim();
            item.set_derived_url(url, source.clone());
            if url.starts_with("file:") {
                item.set_derived_file_url(url, source);
            }
        }
    }
    item.set_native(NativeRepresentation::named(mime, bytes));
}

fn read_pipe_bounded(
    reader: &mut impl ReadAndFd,
    max_bytes: Option<u64>,
) -> Result<Option<Vec<u8>>> {
    let fd = reader.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error())
            .context("make Wayland clipboard pipe nonblocking");
    }

    let deadline = Instant::now() + SELECTION_TIMEOUT;
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(Some(bytes)),
            Ok(read) => {
                if max_bytes.is_some_and(|limit| {
                    u128::try_from(bytes.len())
                        .unwrap_or(u128::MAX)
                        .saturating_add(read as u128)
                        > u128::from(limit)
                }) {
                    return Ok(None);
                }
                bytes.extend_from_slice(&chunk[..read]);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                wait_for_pipe(fd, deadline)?;
            }
            Err(error) => return Err(error).context("read Wayland clipboard pipe"),
        }
    }
}

trait ReadAndFd: Read + AsRawFd {}
impl<T: Read + AsRawFd> ReadAndFd for T {}

fn wait_for_pipe(fd: i32, deadline: Instant) -> Result<()> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        anyhow::bail!(
            "timed out after {}s waiting for Wayland clipboard data",
            SELECTION_TIMEOUT.as_secs()
        );
    }
    let timeout = i32::try_from(remaining.as_millis().max(1)).unwrap_or(i32::MAX);
    let mut descriptor = libc::pollfd {
        fd,
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut descriptor, 1, timeout) };
    if ready < 0 {
        return Err(std::io::Error::last_os_error()).context("wait for Wayland clipboard data");
    }
    if ready == 0 {
        anyhow::bail!(
            "timed out after {}s waiting for Wayland clipboard data",
            SELECTION_TIMEOUT.as_secs()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mime_remains_native_while_semantics_use_first_candidate() {
        let mut item = ClipboardItem::for_platform(ClipboardPlatform::Wayland);
        add_representation(&mut item, "text/plain", b"first".to_vec());
        add_representation(
            &mut item,
            "text/plain;charset=windows-1252",
            b"caf\xe9".to_vec(),
        );
        add_representation(
            &mut item,
            "text/uri-list",
            b"file:///tmp/example.txt\n".to_vec(),
        );

        assert_eq!(item.text(), Some("first"));
        assert_eq!(item.url(), Some("file:///tmp/example.txt"));
        assert_eq!(item.file_url(), Some("file:///tmp/example.txt"));
        assert_eq!(
            item.representation("text/plain;charset=windows-1252")
                .unwrap()
                .data(),
            b"caf\xe9"
        );
    }
}
