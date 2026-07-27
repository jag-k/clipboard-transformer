#[cfg(any(target_os = "windows", test))]
pub mod cf_html;
mod item;
#[cfg(feature = "native")]
pub mod native;

use std::collections::BTreeSet;

use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[doc(hidden)]
pub use item::ClipboardFingerprint;
pub use item::{
    decode_latin1, decode_mime_text, normalize_format, ClipboardFormat, ClipboardItem,
    ClipboardPlatform, NativeFormatFlag, NativeRepresentation, SemanticValue, SemanticViews,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClipboardSourceApp {
    pub bundle_id: Option<String>,
    pub name: Option<String>,
}

impl ClipboardSourceApp {
    pub fn new(bundle_id: Option<String>, name: Option<String>) -> Self {
        Self { bundle_id, name }
    }

    pub fn matches_any(&self, apps: &[String]) -> bool {
        apps.iter().any(|app| self.matches(app))
    }

    fn matches(&self, app: &str) -> bool {
        let app = app.trim();
        if app.is_empty() {
            return false;
        }

        self.bundle_id
            .as_deref()
            .is_some_and(|bundle_id| bundle_id.eq_ignore_ascii_case(app))
            || self
                .name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(app))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClipboardMetadata {
    ignored: bool,
    source_app: Option<ClipboardSourceApp>,
}

impl ClipboardMetadata {
    pub fn readable(source_app: Option<ClipboardSourceApp>) -> Self {
        Self {
            ignored: false,
            source_app,
        }
    }

    pub fn ignored() -> Self {
        Self {
            ignored: true,
            source_app: None,
        }
    }

    pub fn is_ignored(&self) -> bool {
        self.ignored
    }

    pub fn source_app(&self) -> Option<&ClipboardSourceApp> {
        self.source_app.as_ref()
    }
}

pub trait ClipboardBackend {
    fn change_count(&mut self) -> Result<Option<u64>> {
        Ok(None)
    }

    fn read(&mut self) -> Result<Option<ClipboardItem>>;

    /// Reads identifiers and best-effort source metadata without materializing
    /// payload values, except for platform flags whose value defines whether
    /// the item must be excluded.
    fn metadata(&mut self) -> Result<ClipboardMetadata> {
        Ok(ClipboardMetadata::default())
    }

    fn read_limited(&mut self, max_bytes: u64) -> Result<Option<ClipboardItem>> {
        let item = self.read()?;
        Ok(
            item.filter(|item| {
                max_bytes == 0 || item.size_bytes() as u128 <= u128::from(max_bytes)
            }),
        )
    }

    /// Reads only representations requested by compiled rules when the native
    /// backend can avoid materializing unrelated payloads.
    fn read_formats_limited(
        &mut self,
        formats: &BTreeSet<ClipboardFormat>,
        max_bytes: u64,
    ) -> Result<Option<ClipboardItem>> {
        let mut item = self.read_limited(max_bytes)?;
        if let Some(item) = &mut item {
            item.retain_formats(formats);
        }
        Ok(item.filter(|item| formats.iter().any(|format| item.bytes(format).is_some())))
    }

    /// Reads an item only when its native type metadata says it is suitable
    /// for monitoring and transformation.
    fn read_transformable(&mut self, max_bytes: u64) -> Result<Option<ClipboardItem>> {
        if self.metadata()?.is_ignored() {
            return Ok(None);
        }
        self.read_limited(max_bytes)
    }

    fn write(&mut self, content: &ClipboardItem) -> Result<()>;
}

impl<T> ClipboardBackend for Box<T>
where
    T: ClipboardBackend + ?Sized,
{
    fn change_count(&mut self) -> Result<Option<u64>> {
        (**self).change_count()
    }

    fn read(&mut self) -> Result<Option<ClipboardItem>> {
        (**self).read()
    }

    fn metadata(&mut self) -> Result<ClipboardMetadata> {
        (**self).metadata()
    }

    fn read_limited(&mut self, max_bytes: u64) -> Result<Option<ClipboardItem>> {
        (**self).read_limited(max_bytes)
    }

    fn read_formats_limited(
        &mut self,
        formats: &BTreeSet<ClipboardFormat>,
        max_bytes: u64,
    ) -> Result<Option<ClipboardItem>> {
        (**self).read_formats_limited(formats, max_bytes)
    }

    fn read_transformable(&mut self, max_bytes: u64) -> Result<Option<ClipboardItem>> {
        (**self).read_transformable(max_bytes)
    }

    fn write(&mut self, content: &ClipboardItem) -> Result<()> {
        (**self).write(content)
    }
}

#[derive(Debug, Default)]
pub struct MemoryClipboardBackend {
    current: Option<ClipboardItem>,
}

impl MemoryClipboardBackend {
    pub fn new(current: Option<ClipboardItem>) -> Self {
        Self { current }
    }
}

impl ClipboardBackend for MemoryClipboardBackend {
    fn metadata(&mut self) -> Result<ClipboardMetadata> {
        Ok(ClipboardMetadata::readable(
            self.current
                .as_ref()
                .and_then(|item| item.source_app().cloned()),
        ))
    }

    fn read(&mut self) -> Result<Option<ClipboardItem>> {
        Ok(self.current.clone())
    }

    fn write(&mut self, content: &ClipboardItem) -> Result<()> {
        self.current = Some(content.clone());
        Ok(())
    }
}

pub fn format_requests_native_kind(formats: &BTreeSet<ClipboardFormat>, kind: &str) -> bool {
    if formats.iter().any(|format| format.as_str() == kind) {
        return true;
    }
    let lower = kind.to_ascii_lowercase();
    let base = lower
        .split_once(';')
        .map_or(lower.as_str(), |(base, _)| base)
        .trim();
    formats.iter().any(|format| match format.as_str() {
        "text" => {
            base == "text/plain"
                || matches!(
                    base,
                    "public.utf8-plain-text"
                        | "public.utf16-plain-text"
                        | "nsstringpboardtype"
                        | "utf8_string"
                        | "text"
                        | "string"
                        | "cf_unicodetext"
                )
        }
        "url" => {
            base == "text/uri-list"
                || matches!(
                    base,
                    "public.url"
                        | "public.file-url"
                        | "uniformresourcelocator"
                        | "uniformresourcelocatorw"
                )
        }
        "html" => matches!(base, "public.html" | "text/html"),
        "rtf" => {
            matches!(
                base,
                "public.rtf" | "text/rtf" | "application/rtf" | "rich text format"
            )
        }
        "file-url" => matches!(base, "public.file-url" | "text/uri-list"),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct IgnoredClipboard;

    impl ClipboardBackend for IgnoredClipboard {
        fn metadata(&mut self) -> Result<ClipboardMetadata> {
            Ok(ClipboardMetadata::ignored())
        }

        fn read(&mut self) -> Result<Option<ClipboardItem>> {
            panic!("ignored clipboard payload must not be read")
        }

        fn write(&mut self, _content: &ClipboardItem) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn default_limited_read_rejects_oversized_items() {
        let mut backend = MemoryClipboardBackend::new(Some(ClipboardItem::from_text("cat")));

        assert!(backend.read_limited(2).unwrap().is_none());
        assert_eq!(
            backend.read_limited(0).unwrap().unwrap().text(),
            Some("cat")
        );
    }

    #[test]
    fn transformable_read_stops_after_ignored_metadata() {
        assert!(IgnoredClipboard.read_transformable(0).unwrap().is_none());
    }

    #[test]
    fn native_format_selection_understands_semantic_aliases() {
        let formats = BTreeSet::from([
            ClipboardFormat::new("text"),
            ClipboardFormat::new("public.png"),
        ]);

        assert!(format_requests_native_kind(
            &formats,
            "text/plain;charset=utf-8"
        ));
        assert!(format_requests_native_kind(&formats, "public.png"));
        assert!(!format_requests_native_kind(&formats, "public.html"));
    }

    /// Each semantic format must match its own platform aliases and reject
    /// every other kind. This table is easy to break silently, so keep the
    /// negative cases alongside the positive ones.
    #[test]
    fn semantic_formats_select_only_matching_native_kinds() {
        let text = BTreeSet::from([ClipboardFormat::text()]);
        for kind in [
            "public.utf8-plain-text",
            "public.utf16-plain-text",
            "NSStringPboardType",
            "UTF8_STRING",
            "STRING",
            "CF_UNICODETEXT",
            "text/plain",
            "text/plain;charset=utf-8",
        ] {
            assert!(
                format_requests_native_kind(&text, kind),
                "text should match {kind}"
            );
        }
        for kind in ["public.png", "public.html", "text/html", "public.rtf"] {
            assert!(
                !format_requests_native_kind(&text, kind),
                "text must not match {kind}"
            );
        }

        let url = BTreeSet::from([ClipboardFormat::url()]);
        for kind in [
            "public.url",
            "public.file-url",
            "text/uri-list",
            "UniformResourceLocator",
            "UniformResourceLocatorW",
        ] {
            assert!(
                format_requests_native_kind(&url, kind),
                "url should match {kind}"
            );
        }
        assert!(!format_requests_native_kind(&url, "public.tiff"));
        assert!(!format_requests_native_kind(&url, "text/plain"));

        let html = BTreeSet::from([ClipboardFormat::html()]);
        assert!(format_requests_native_kind(&html, "public.html"));
        assert!(format_requests_native_kind(
            &html,
            "text/html; charset=windows-1252"
        ));
        assert!(!format_requests_native_kind(&html, "text/plain"));

        let rtf = BTreeSet::from([ClipboardFormat::rtf()]);
        for kind in [
            "public.rtf",
            "text/rtf",
            "application/rtf;charset=utf-8",
            "Rich Text Format",
        ] {
            assert!(
                format_requests_native_kind(&rtf, kind),
                "rtf should match {kind}"
            );
        }
        assert!(!format_requests_native_kind(&rtf, "public.html"));

        let file_url = BTreeSet::from([ClipboardFormat::new("file-url")]);
        assert!(format_requests_native_kind(&file_url, "public.file-url"));
        assert!(format_requests_native_kind(&file_url, "text/uri-list"));
        assert!(!format_requests_native_kind(&file_url, "public.url"));

        // A native identifier matches itself exactly and nothing else.
        let native = BTreeSet::from([ClipboardFormat::new("public.png")]);
        assert!(format_requests_native_kind(&native, "public.png"));
        assert!(!format_requests_native_kind(&native, "public.tiff"));
    }

    #[test]
    fn native_kind_matching_ignores_case_and_parameter_whitespace() {
        let html = BTreeSet::from([ClipboardFormat::html()]);
        assert!(format_requests_native_kind(&html, "TEXT/HTML"));
        assert!(format_requests_native_kind(
            &html,
            "text/html ; charset=utf-8"
        ));
    }

    #[test]
    fn empty_format_set_requests_nothing() {
        let none = BTreeSet::new();
        assert!(!format_requests_native_kind(&none, "text/plain"));
        assert!(!format_requests_native_kind(
            &none,
            "public.utf8-plain-text"
        ));
    }
}
