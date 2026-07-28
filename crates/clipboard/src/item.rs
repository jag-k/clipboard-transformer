use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use encoding_rs::Encoding;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A portable rule-facing selector.
///
/// `ClipboardFormat` is deliberately separate from [`NativeRepresentation`].
/// Common values such as `text` and `html` select semantic views; every other
/// value selects a native representation by its exact [`NativeRepresentation::kind`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ClipboardFormat(String);

impl ClipboardFormat {
    /// Creates a semantic selector or an exact, case-sensitive native kind.
    pub fn new(format: impl Into<String>) -> Self {
        Self(format.into())
    }

    /// Returns the canonical semantic alias or exact native kind.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Selects semantic plain text.
    pub fn text() -> Self {
        Self::new("text")
    }

    /// Selects a semantic URL.
    pub fn url() -> Self {
        Self::new("url")
    }

    /// Selects semantic HTML.
    pub fn html() -> Self {
        Self::new("html")
    }

    /// Selects semantic RTF source.
    pub fn rtf() -> Self {
        Self::new("rtf")
    }

    /// Selects a semantic file URL.
    pub fn file_url() -> Self {
        Self::new("file-url")
    }
}

/// The native system from which a clipboard snapshot was captured.
///
/// A transformed item retains its source platform so the matching native
/// backend can interpret its descriptors and encode authored semantic
/// overrides. `Portable` is used for items created without a native clipboard,
/// for example by the stdin transform command.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ClipboardPlatform {
    /// No system clipboard is involved, for example stdin transform input.
    #[default]
    Portable,
    /// Apple pasteboard types and AppKit conversion rules.
    Macos,
    /// Win32 predefined and registered clipboard formats.
    Windows,
    /// Wayland data-control MIME offers.
    Wayland,
    /// X11 selection targets and returned properties.
    X11,
}

impl ClipboardPlatform {
    /// Returns the stable serde/plugin spelling of this platform.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Portable => "portable",
            Self::Macos => "macos",
            Self::Windows => "windows",
            Self::Wayland => "wayland",
            Self::X11 => "x11",
        }
    }
}

impl std::fmt::Display for ClipboardPlatform {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Additional native facts which do not belong in a representation's name.
///
/// Windows format classes and the X11 INCR transport are intentionally flags:
/// the exact native identity remains in `kind`/`id`, and a restored item may
/// use a different transport while preserving the same content.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum NativeFormatFlag {
    /// A Win32 format whose numeric id is defined by the operating system.
    Predefined,
    /// A named Win32 format registered dynamically for the current session.
    Registered,
    /// A Win32 private format in the `CF_PRIVATEFIRST..=CF_PRIVATELAST` range.
    Private,
    /// A Win32 GDI-object format in the `CF_GDIOBJFIRST..=CF_GDIOBJLAST` range.
    GdiObject,
    /// The X11 payload was observed through the INCR transfer protocol.
    Incremental,
}

/// One exact native clipboard representation and its raw payload.
///
/// The fields have platform-specific invariants:
///
/// - macOS: `kind` is the exact pasteboard type; the other descriptor fields
///   are absent.
/// - Wayland: `kind` is the exact offered MIME string, including parameters.
/// - Windows: `kind` is a predefined `CF_*` name or exact registered name,
///   `id` is the observed format id, and a class flag distinguishes them.
///   Registered ids are session-local; writers re-register by `kind`.
/// - X11: `kind` is the requested target atom name, `returned_type` is the
///   actual property type selected by the owner, and `unit_bits` is 8, 16, or
///   32. `Incremental` records that INCR was used, but is not replayed blindly.
///
/// `data` is never a decoded semantic value. It is the raw native byte
/// sequence retained for inspection, history, undo, and lossless restoration.
///
/// ```
/// use ct_clipboard::{
///     ClipboardPlatform, NativeRepresentation,
/// };
///
/// let representation = NativeRepresentation::x11(
///     "TEXT",
///     "COMPOUND_TEXT",
///     8,
///     b"hello".to_vec(),
///     false,
/// );
/// assert_eq!(representation.kind(), "TEXT");
/// assert_eq!(representation.returned_type(), Some("COMPOUND_TEXT"));
/// assert_eq!(representation.unit_bits(), Some(8));
/// let _platform = ClipboardPlatform::X11;
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct NativeRepresentation {
    /// Exact native identity: pasteboard type, MIME, Windows name, or requested
    /// X11 target.
    kind: String,
    /// Observed Win32 format id. It is diagnostic for registered formats,
    /// whose id can change after restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<u32>,
    /// Platform facts which refine, but never replace, `kind`.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    flags: BTreeSet<NativeFormatFlag>,
    /// Actual X11 property type returned by the selection owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    returned_type: Option<String>,
    /// X11 property element width: exactly 8, 16, or 32 bits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unit_bits: Option<u8>,
    /// Unmodified native payload bytes.
    #[serde(with = "serde_bytes")]
    #[schemars(with = "Vec<u8>")]
    data: Vec<u8>,
}

impl NativeRepresentation {
    /// Creates a macOS- or Wayland-style named representation.
    ///
    /// This constructor does not infer a platform or semantic meaning. The
    /// containing [`ClipboardItem`] supplies the platform, while its semantic
    /// views are populated separately.
    pub fn named(kind: impl Into<String>, data: Vec<u8>) -> Self {
        Self {
            kind: kind.into(),
            id: None,
            flags: BTreeSet::new(),
            returned_type: None,
            unit_bits: None,
            data,
        }
    }

    /// Creates a representation for an operating-system-defined Win32 format.
    pub fn windows_predefined(kind: impl Into<String>, id: u32, data: Vec<u8>) -> Self {
        Self {
            kind: kind.into(),
            id: Some(id),
            flags: [NativeFormatFlag::Predefined].into(),
            returned_type: None,
            unit_bits: None,
            data,
        }
    }

    /// Creates a representation for a named Win32 registered format.
    pub fn windows_registered(kind: impl Into<String>, id: u32, data: Vec<u8>) -> Self {
        Self {
            kind: kind.into(),
            id: Some(id),
            flags: [NativeFormatFlag::Registered].into(),
            returned_type: None,
            unit_bits: None,
            data,
        }
    }

    /// Creates an X11 representation with both requested and returned types.
    ///
    /// `target` is what the requestor asked the selection owner to convert;
    /// `returned_type` and `unit_bits` describe what the owner actually placed
    /// on the requestor property. `incremental` records observed transport only.
    pub fn x11(
        target: impl Into<String>,
        returned_type: impl Into<String>,
        unit_bits: u8,
        data: Vec<u8>,
        incremental: bool,
    ) -> Self {
        debug_assert!(matches!(unit_bits, 8 | 16 | 32));
        Self {
            kind: target.into(),
            id: None,
            flags: incremental
                .then_some(NativeFormatFlag::Incremental)
                .into_iter()
                .collect(),
            returned_type: Some(returned_type.into()),
            unit_bits: Some(unit_bits),
            data,
        }
    }

    /// Returns the exact native identity, without alias normalization.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Returns the observed Win32 format id, when applicable.
    pub fn id(&self) -> Option<u32> {
        self.id
    }

    /// Returns platform-specific classification and transport facts.
    pub fn flags(&self) -> &BTreeSet<NativeFormatFlag> {
        &self.flags
    }

    /// Returns the actual X11 property type, when applicable.
    pub fn returned_type(&self) -> Option<&str> {
        self.returned_type.as_deref()
    }

    /// Returns the X11 property element width, when applicable.
    pub fn unit_bits(&self) -> Option<u8> {
        self.unit_bits
    }

    /// Returns the unmodified native payload.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub(crate) fn into_data(self) -> Vec<u8> {
        self.data
    }
}

/// One effective portable semantic value.
///
/// A derived value is a UTF-8 view decoded from the native representations in
/// `derived_from`. An authored value is an override created by a rule or a
/// portable caller. Writers encode authored values into appropriate native
/// formats and must not replay stale native representations for that semantic.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct SemanticValue {
    /// Effective UTF-8 value exposed to portable rules.
    value: String,
    /// Whether the value was explicitly authored rather than decoded.
    authored: bool,
    /// Exact native kinds from which a backend derived this value.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    derived_from: Vec<String>,
}

impl SemanticValue {
    /// Creates a semantic view decoded from one or more native kinds.
    pub fn derived(value: impl Into<String>, derived_from: Vec<String>) -> Self {
        Self {
            value: value.into(),
            authored: false,
            derived_from,
        }
    }

    /// Creates a semantic override that a native writer must encode.
    pub fn authored(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            authored: true,
            derived_from: Vec::new(),
        }
    }

    /// Returns the effective UTF-8 value.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Reports whether this is an authored override.
    pub fn is_authored(&self) -> bool {
        self.authored
    }

    /// Returns exact native kinds that contributed to a derived value.
    pub fn derived_from(&self) -> &[String] {
        &self.derived_from
    }
}

/// Portable UTF-8 views and overrides over native clipboard representations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct SemanticViews {
    /// Plain text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<SemanticValue>,
    /// A general URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<SemanticValue>,
    /// An HTML fragment or document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    html: Option<SemanticValue>,
    /// Rich Text Format source represented as UTF-8 text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rtf: Option<SemanticValue>,
    /// A file URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file_url: Option<SemanticValue>,
}

/// A complete clipboard snapshot.
///
/// Native representations retain their original platform identity, descriptor
/// fields, order, and raw bytes. Semantic values are separate UTF-8
/// views/overrides used by portable rules. This separation prevents a Wayland
/// MIME offer, Windows format, or X11 reply from being renamed into a
/// macOS-shaped `public.*` key.
///
/// ```
/// use ct_clipboard::{
///     decode_mime_text, ClipboardItem, ClipboardPlatform, NativeRepresentation,
/// };
///
/// let mime = "text/plain;charset=windows-1252";
/// let raw = b"caf\xe9".to_vec();
/// let mut item = ClipboardItem::for_platform(ClipboardPlatform::Wayland);
/// item.set_native(NativeRepresentation::named(mime, raw.clone()));
/// item.set_derived_text(decode_mime_text(mime, &raw).unwrap(), vec![mime.into()]);
///
/// assert_eq!(item.text(), Some("caf\u{e9}"));
/// assert_eq!(item.representation(mime).unwrap().data(), raw);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClipboardItem {
    /// Native system that defines every representation descriptor.
    platform: ClipboardPlatform,
    /// Ordered exact native representations.
    #[serde(with = "representation_list")]
    #[schemars(with = "Vec<NativeRepresentation>")]
    representations: Arc<Vec<NativeRepresentation>>,
    /// Portable UTF-8 views and authored overrides.
    #[serde(default)]
    semantics: SemanticViews,
    /// Best-effort application metadata; absent when the platform cannot
    /// attribute clipboard ownership.
    source_app: Option<crate::ClipboardSourceApp>,
}

/// Compact process-local identity for a complete clipboard payload.
///
/// Source-application metadata is intentionally excluded to match
/// [`ClipboardItem::payload_eq`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub struct ClipboardFingerprint {
    payload: u64,
    text: Option<u64>,
}

impl ClipboardFingerprint {
    #[doc(hidden)]
    pub fn matches_own_write(self, current: Self) -> bool {
        self.payload == current.payload
            || self
                .text
                .zip(current.text)
                .is_some_and(|(expected, current)| expected == current)
    }
}

impl ClipboardItem {
    /// Creates an empty portable item.
    pub fn new() -> Self {
        Self::for_platform(ClipboardPlatform::Portable)
    }

    /// Creates an empty item whose descriptors belong to `platform`.
    pub fn for_platform(platform: ClipboardPlatform) -> Self {
        Self {
            platform,
            representations: Arc::new(Vec::new()),
            semantics: SemanticViews::default(),
            source_app: None,
        }
    }

    /// Creates a portable item with authored plain text.
    pub fn from_text(text: impl Into<String>) -> Self {
        let mut item = Self::new();
        item.set_text(text);
        item
    }

    /// Returns the native system that defines the representation descriptors.
    pub fn platform(&self) -> ClipboardPlatform {
        self.platform
    }

    /// Returns a semantic UTF-8 view or an exact native representation decoded
    /// as UTF-8.
    pub fn get(&self, format: &ClipboardFormat) -> Option<&str> {
        match format.as_str() {
            "text" => self.text(),
            "url" => self.url(),
            "html" => self.html(),
            "rtf" => self.rtf(),
            "file-url" => self.semantics.file_url.as_ref().map(SemanticValue::value),
            kind => self
                .representation(kind)
                .and_then(|representation| std::str::from_utf8(representation.data()).ok()),
        }
    }

    /// Sets an authored semantic value or a UTF-8 named native representation.
    pub fn set(&mut self, format: ClipboardFormat, value: String) {
        match format.as_str() {
            "text" => self.set_text(value),
            "url" => self.set_url(value),
            "html" => self.set_html(value),
            "rtf" => self.set_rtf(value),
            "file-url" => self.semantics.file_url = Some(SemanticValue::authored(value)),
            kind => self.set_native(NativeRepresentation::named(kind, value.into_bytes())),
        }
    }

    /// Returns a semantic value as UTF-8 bytes or exact native bytes.
    pub fn bytes(&self, format: &ClipboardFormat) -> Option<&[u8]> {
        match format.as_str() {
            "text" => self.text().map(str::as_bytes),
            "url" => self.url().map(str::as_bytes),
            "html" => self.html().map(str::as_bytes),
            "rtf" => self.rtf().map(str::as_bytes),
            "file-url" => self
                .semantics
                .file_url
                .as_ref()
                .map(SemanticValue::value)
                .map(str::as_bytes),
            kind => self.representation(kind).map(NativeRepresentation::data),
        }
    }

    /// Sets bytes under a semantic selector or exact native kind.
    ///
    /// Valid UTF-8 semantic bytes become authored semantic values. Invalid
    /// UTF-8 cannot be semantic and is retained as an exact named native
    /// representation instead.
    pub fn set_bytes(&mut self, format: ClipboardFormat, value: Vec<u8>) {
        match String::from_utf8(value) {
            Ok(value) => self.set(format, value),
            Err(error) => self.set_native(NativeRepresentation::named(
                format.as_str(),
                error.into_bytes(),
            )),
        }
    }

    /// Removes a semantic value or the native representation with exact `kind`.
    pub fn remove(&mut self, format: &ClipboardFormat) -> Option<Vec<u8>> {
        match format.as_str() {
            "text" => self.semantics.text.take(),
            "url" => self.semantics.url.take(),
            "html" => self.semantics.html.take(),
            "rtf" => self.semantics.rtf.take(),
            "file-url" => self.semantics.file_url.take(),
            kind => {
                let index = self
                    .representations
                    .iter()
                    .position(|representation| representation.kind() == kind)?;
                return Some(
                    Arc::make_mut(&mut self.representations)
                        .remove(index)
                        .into_data(),
                );
            }
        }
        .map(|value| value.value.into_bytes())
    }

    /// Returns the effective UTF-8 plain-text view or override.
    pub fn text(&self) -> Option<&str> {
        self.semantics.text.as_ref().map(SemanticValue::value)
    }

    /// Authors a UTF-8 plain-text override without discarding the snapshot.
    pub fn set_text(&mut self, value: impl Into<String>) {
        self.semantics.text = Some(SemanticValue::authored(value));
    }

    /// Replaces the complete item with authored plain text.
    ///
    /// A text transform must not leave stale HTML, rich text, images, or
    /// application-specific representations beside the new value: paste
    /// targets could otherwise observe mutually inconsistent content.
    pub fn replace_with_text(&mut self, value: impl Into<String>) {
        self.representations = Arc::new(Vec::new());
        self.semantics = SemanticViews {
            text: Some(SemanticValue::authored(value)),
            ..SemanticViews::default()
        };
    }

    /// Returns the effective UTF-8 URL view or override.
    pub fn url(&self) -> Option<&str> {
        self.semantics.url.as_ref().map(SemanticValue::value)
    }

    /// Authors a UTF-8 URL override without discarding the snapshot.
    pub fn set_url(&mut self, value: impl Into<String>) {
        self.semantics.url = Some(SemanticValue::authored(value));
    }

    /// Returns the effective UTF-8 HTML view or override.
    pub fn html(&self) -> Option<&str> {
        self.semantics.html.as_ref().map(SemanticValue::value)
    }

    /// Authors a UTF-8 HTML override without discarding the snapshot.
    pub fn set_html(&mut self, value: impl Into<String>) {
        self.semantics.html = Some(SemanticValue::authored(value));
    }

    /// Returns the effective UTF-8 RTF source view or override.
    pub fn rtf(&self) -> Option<&str> {
        self.semantics.rtf.as_ref().map(SemanticValue::value)
    }

    /// Authors a UTF-8 RTF source override without discarding the snapshot.
    pub fn set_rtf(&mut self, value: impl Into<String>) {
        self.semantics.rtf = Some(SemanticValue::authored(value));
    }

    /// Returns the effective UTF-8 file-URL view or override.
    pub fn file_url(&self) -> Option<&str> {
        self.semantics.file_url.as_ref().map(SemanticValue::value)
    }

    /// Authors a UTF-8 file-URL override without discarding the snapshot.
    pub fn set_file_url(&mut self, value: impl Into<String>) {
        self.semantics.file_url = Some(SemanticValue::authored(value));
    }

    /// Returns all portable semantic values.
    pub fn semantics(&self) -> &SemanticViews {
        &self.semantics
    }

    /// Returns plain text together with authored/derivation metadata.
    pub fn text_semantic(&self) -> Option<&SemanticValue> {
        self.semantics.text.as_ref()
    }

    /// Returns the URL together with authored/derivation metadata.
    pub fn url_semantic(&self) -> Option<&SemanticValue> {
        self.semantics.url.as_ref()
    }

    /// Returns HTML together with authored/derivation metadata.
    pub fn html_semantic(&self) -> Option<&SemanticValue> {
        self.semantics.html.as_ref()
    }

    /// Returns RTF together with authored/derivation metadata.
    pub fn rtf_semantic(&self) -> Option<&SemanticValue> {
        self.semantics.rtf.as_ref()
    }

    /// Returns the file URL together with authored/derivation metadata.
    pub fn file_url_semantic(&self) -> Option<&SemanticValue> {
        self.semantics.file_url.as_ref()
    }

    /// Sets the first derived plain-text candidate.
    pub fn set_derived_text(&mut self, value: impl Into<String>, from: Vec<String>) {
        if self.semantics.text.is_none() {
            self.semantics.text = Some(SemanticValue::derived(value, from));
        }
    }

    /// Sets the first derived URL candidate.
    pub fn set_derived_url(&mut self, value: impl Into<String>, from: Vec<String>) {
        if self.semantics.url.is_none() {
            self.semantics.url = Some(SemanticValue::derived(value, from));
        }
    }

    /// Sets the first derived HTML candidate.
    pub fn set_derived_html(&mut self, value: impl Into<String>, from: Vec<String>) {
        if self.semantics.html.is_none() {
            self.semantics.html = Some(SemanticValue::derived(value, from));
        }
    }

    /// Sets the first derived RTF candidate.
    pub fn set_derived_rtf(&mut self, value: impl Into<String>, from: Vec<String>) {
        if self.semantics.rtf.is_none() {
            self.semantics.rtf = Some(SemanticValue::derived(value, from));
        }
    }

    /// Sets the first derived file-URL candidate.
    pub fn set_derived_file_url(&mut self, value: impl Into<String>, from: Vec<String>) {
        if self.semantics.file_url.is_none() {
            self.semantics.file_url = Some(SemanticValue::derived(value, from));
        }
    }

    /// Returns all exact native representations in observed preference order.
    pub fn representations(&self) -> &[NativeRepresentation] {
        &self.representations
    }

    /// Finds a native representation by exact, case-sensitive `kind`.
    pub fn representation(&self, kind: &str) -> Option<&NativeRepresentation> {
        self.representations
            .iter()
            .find(|representation| representation.kind() == kind)
    }

    /// Adds a native representation while preserving observed order.
    ///
    /// Native APIs expose at most one payload for one kind in a clipboard
    /// item. A repeated kind replaces the earlier payload in place so its
    /// original preference position remains stable.
    pub fn set_native(&mut self, representation: NativeRepresentation) {
        let representations = Arc::make_mut(&mut self.representations);
        if let Some(existing) = representations
            .iter_mut()
            .find(|existing| existing.kind() == representation.kind())
        {
            *existing = representation;
        } else {
            representations.push(representation);
        }
    }

    pub fn payload_eq(&self, other: &Self) -> bool {
        self.platform == other.platform
            && self.representations == other.representations
            && self.semantics == other.semantics
    }

    #[doc(hidden)]
    pub fn fingerprint(&self) -> ClipboardFingerprint {
        let mut hasher = DefaultHasher::new();
        self.platform.hash(&mut hasher);
        self.representations.hash(&mut hasher);
        self.semantics.hash(&mut hasher);
        ClipboardFingerprint {
            payload: hasher.finish(),
            text: self.text().map(|text| {
                let mut hasher = DefaultHasher::new();
                text.hash(&mut hasher);
                hasher.finish()
            }),
        }
    }

    /// Retains only selected semantics and exact native kinds.
    pub fn retain_formats(&mut self, formats: &BTreeSet<ClipboardFormat>) {
        if !formats.contains(&ClipboardFormat::text()) {
            self.semantics.text = None;
        }
        if !formats.contains(&ClipboardFormat::url()) {
            self.semantics.url = None;
        }
        if !formats.contains(&ClipboardFormat::html()) {
            self.semantics.html = None;
        }
        if !formats.contains(&ClipboardFormat::rtf()) {
            self.semantics.rtf = None;
        }
        if !formats.contains(&ClipboardFormat::file_url()) {
            self.semantics.file_url = None;
        }
        Arc::make_mut(&mut self.representations).retain(|representation| {
            formats.contains(&ClipboardFormat::new(representation.kind()))
        });
    }

    /// Raw native bytes plus authored portable values.
    ///
    /// Derived semantic views are decoded caches over raw data and are not
    /// counted twice. Authored values have no native payload yet and therefore
    /// participate in item/history limits.
    pub fn size_bytes(&self) -> usize {
        let native = self.representations.iter().fold(0usize, |total, value| {
            total.saturating_add(value.data().len())
        });
        [
            self.semantics.text.as_ref(),
            self.semantics.url.as_ref(),
            self.semantics.html.as_ref(),
            self.semantics.rtf.as_ref(),
            self.semantics.file_url.as_ref(),
        ]
        .into_iter()
        .flatten()
        .filter(|value| value.is_authored())
        .fold(native, |total, value| {
            total.saturating_add(value.value().len())
        })
    }

    /// Iterates over available portable semantic text values.
    pub fn text_representations(&self) -> impl Iterator<Item = (ClipboardFormat, &str)> {
        [
            (ClipboardFormat::text(), self.text()),
            (ClipboardFormat::url(), self.url()),
            (ClipboardFormat::html(), self.html()),
            (ClipboardFormat::rtf(), self.rtf()),
            (ClipboardFormat::file_url(), self.file_url()),
        ]
        .into_iter()
        .filter_map(|(format, value)| value.map(|value| (format, value)))
    }

    /// Returns best-effort source-application metadata.
    pub fn source_app(&self) -> Option<&crate::ClipboardSourceApp> {
        self.source_app.as_ref()
    }

    /// Attaches best-effort source-application metadata.
    pub fn with_source_app(mut self, app: crate::ClipboardSourceApp) -> Self {
        self.source_app = Some(app);
        self
    }

    #[doc(hidden)]
    pub fn with_optional_source_app(mut self, app: Option<crate::ClipboardSourceApp>) -> Self {
        self.source_app = app;
        self
    }
}

/// Decodes a textual MIME payload according to its explicit `charset`.
///
/// The raw bytes and exact MIME string remain in the native representation.
/// Missing `charset` means UTF-8. Unknown labels and malformed input return
/// `None`, allowing callers to try another offered text representation.
///
/// ```
/// use ct_clipboard::decode_mime_text;
///
/// assert_eq!(
///     decode_mime_text("text/plain;charset=windows-1252", b"caf\xe9"),
///     Some("caf\u{e9}".to_string()),
/// );
/// ```
pub fn decode_mime_text(mime: &str, bytes: &[u8]) -> Option<String> {
    let charset = mime.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['"', '\'']))
    });
    let encoding = match charset {
        Some(label) => Encoding::for_label(label.as_bytes())?,
        None => encoding_rs::UTF_8,
    };
    let (decoded, had_errors) = encoding.decode_without_bom_handling(bytes);
    (!had_errors).then(|| decoded.into_owned())
}

/// Losslessly maps X11 `STRING` bytes (ISO-8859-1) into Unicode.
pub fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| char::from(*byte)).collect()
}

mod representation_list {
    use std::sync::Arc;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::NativeRepresentation;

    pub fn serialize<S>(
        values: &Arc<Vec<NativeRepresentation>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        values.as_slice().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Arc<Vec<NativeRepresentation>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Vec::<NativeRepresentation>::deserialize(deserializer).map(Arc::new)
    }
}

impl Default for ClipboardItem {
    fn default() -> Self {
        Self::new()
    }
}

/// Normalizes portable aliases while leaving every other native kind exact.
///
/// Legacy macOS-shaped aliases remain accepted as rule syntax only; this
/// function never renames a stored [`NativeRepresentation`].
pub fn normalize_format(format: &str) -> anyhow::Result<ClipboardFormat> {
    let canonical = match format {
        "text" | "plain-text" | "public.utf8-plain-text" => "text",
        "url" | "public.url" => "url",
        "html" | "public.html" => "html",
        "rtf" | "public.rtf" => "rtf",
        "file" | "file-url" | "public.file-url" => "file-url",
        value if value.trim().is_empty() => anyhow::bail!("clipboard format cannot be empty"),
        value => value,
    };
    Ok(ClipboardFormat::new(canonical))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_common_aliases_without_native_names() {
        assert_eq!(normalize_format("text").unwrap().as_str(), "text");
        assert_eq!(
            normalize_format("public.utf8-plain-text").unwrap().as_str(),
            "text"
        );
        assert_eq!(normalize_format("file").unwrap().as_str(), "file-url");
        assert_eq!(normalize_format("html").unwrap().as_str(), "html");
    }

    #[test]
    fn preserves_exact_native_format_identifiers() {
        assert_eq!(
            normalize_format("text/plain;charset=iso-8859-1")
                .unwrap()
                .as_str(),
            "text/plain;charset=iso-8859-1"
        );
        assert_eq!(
            normalize_format("com.example.private-data")
                .unwrap()
                .as_str(),
            "com.example.private-data"
        );
    }

    #[test]
    fn fingerprint_tracks_payload_but_not_source_app() {
        let plain = ClipboardItem::from_text("cat");
        let sourced = plain
            .clone()
            .with_source_app(crate::ClipboardSourceApp::new(
                Some("com.example.Source".into()),
                Some("Source".into()),
            ));
        let changed = ClipboardItem::from_text("dog");

        assert_eq!(plain.fingerprint(), sourced.fingerprint());
        assert_ne!(plain.fingerprint(), changed.fingerprint());
    }

    #[test]
    fn clones_share_payload_until_one_is_mutated() {
        let mut original = ClipboardItem::for_platform(ClipboardPlatform::Wayland);
        original.set_native(NativeRepresentation::named("image/png", vec![1, 2, 3]));
        let mut clone = original.clone();

        assert!(Arc::ptr_eq(
            &original.representations,
            &clone.representations
        ));
        clone.set_native(NativeRepresentation::named("text/plain", b"dog".to_vec()));
        assert!(!Arc::ptr_eq(
            &original.representations,
            &clone.representations
        ));
        assert!(original.representation("text/plain").is_none());
    }

    #[test]
    fn mime_charset_decode_preserves_raw_identity() {
        let mut item = ClipboardItem::for_platform(ClipboardPlatform::Wayland);
        let kind = "text/plain;charset=windows-1252";
        let raw = b"caf\xe9".to_vec();
        item.set_native(NativeRepresentation::named(kind, raw.clone()));
        item.set_derived_text(
            decode_mime_text(kind, &raw).unwrap(),
            vec![kind.to_string()],
        );

        assert_eq!(item.text(), Some("caf\u{e9}"));
        assert_eq!(item.representation(kind).unwrap().data(), raw);
        assert!(!item.text_semantic().unwrap().is_authored());
    }

    #[test]
    fn semantic_override_does_not_replace_native_snapshot() {
        let mut item = ClipboardItem::for_platform(ClipboardPlatform::X11);
        item.set_native(NativeRepresentation::x11(
            "STRING",
            "STRING",
            8,
            b"cat".to_vec(),
            false,
        ));
        item.set_derived_text("cat", vec!["STRING".into()]);
        item.set_text("dog");

        assert_eq!(item.text(), Some("dog"));
        assert_eq!(item.representation("STRING").unwrap().data(), b"cat");
        assert!(item.text_semantic().unwrap().is_authored());
    }

    #[test]
    fn serde_shape_keeps_platform_and_descriptor_fields_structured() {
        let mut item = ClipboardItem::for_platform(ClipboardPlatform::X11);
        item.set_native(NativeRepresentation::x11(
            "TEXT",
            "UTF8_STRING",
            8,
            b"cat".to_vec(),
            true,
        ));
        item.set_derived_text("cat", vec!["TEXT".into()]);

        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(json["platform"], "x11");
        assert_eq!(json["representations"][0]["kind"], "TEXT");
        assert_eq!(json["representations"][0]["returned_type"], "UTF8_STRING");
        assert_eq!(json["representations"][0]["unit_bits"], 8);
        assert_eq!(
            json["representations"][0]["flags"],
            serde_json::json!(["incremental"])
        );
        assert_eq!(
            json["representations"][0]["data"],
            serde_json::json!([99, 97, 116])
        );
        assert_eq!(
            json["semantics"]["text"]["derived_from"],
            serde_json::json!(["TEXT"])
        );
        assert!(json["representations"][0].get("descriptor").is_none());

        let restored: ClipboardItem = serde_json::from_value(json).unwrap();
        assert_eq!(restored, item);
        assert_eq!(restored.platform().to_string(), "x11");
    }
}
