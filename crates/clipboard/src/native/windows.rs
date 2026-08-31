use std::collections::BTreeSet;
use std::path::Path;
use std::ptr;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use windows_sys::Win32::Foundation::{CloseHandle, GlobalFree, HANDLE, HWND};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardFormatNameW, GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use crate::cf_html;
use crate::{format_requests_native_kind, ClipboardBackend, ClipboardMetadata};
use crate::{
    ClipboardFormat, ClipboardItem, ClipboardPlatform, ClipboardSourceApp, NativeFormatFlag,
    NativeRepresentation,
};

const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;
const CF_LOCALE: u32 = 16;
const CF_DIBV5: u32 = 17;
const REGISTERED_FORMAT_MIN: u32 = 0xC000;
const WINDOWS_HTML_FORMAT: &str = "HTML Format";
const WINDOWS_RTF_FORMAT: &str = "Rich Text Format";
const WINDOWS_URL_FORMAT: &str = "UniformResourceLocator";
const WINDOWS_URL_WIDE_FORMAT: &str = "UniformResourceLocatorW";
const SOURCE_MARKER_FORMAT: &str = "dev.jag-k.clipboard-transformer";
const IGNORE_FORMATS: &[&str] = &[
    SOURCE_MARKER_FORMAT,
    "ExcludeClipboardContentFromMonitorProcessing",
    "Clipboard Viewer Ignore",
];
const HISTORY_FORMAT: &str = "CanIncludeInClipboardHistory";
const CLOUD_FORMAT: &str = "ExcludeFromCloudClipboard";
const OPEN_ATTEMPTS: usize = 8;
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(10);

pub struct WindowsClipboardBackend;

impl WindowsClipboardBackend {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }
}

impl ClipboardBackend for WindowsClipboardBackend {
    fn change_count(&mut self) -> Result<Option<u64>> {
        // The system increments this value whenever the clipboard contents change.
        let sequence = unsafe { GetClipboardSequenceNumber() };
        if sequence == 0 {
            anyhow::bail!(
                "GetClipboardSequenceNumber returned zero; clipboard change detection is unavailable"
            );
        }
        Ok(Some(u64::from(sequence)))
    }

    fn read(&mut self) -> Result<Option<ClipboardItem>> {
        self.read_limited(0)
    }

    fn read_limited(&mut self, max_bytes: u64) -> Result<Option<ClipboardItem>> {
        let source_app = foreground_app();
        with_open_clipboard(|| unsafe {
            let mut content = ClipboardItem::for_platform(ClipboardPlatform::Windows);
            let mut total_bytes = 0;
            if !read_raw_representations(&mut content, None, max_bytes, &mut total_bytes)? {
                return Ok(None);
            }
            if !read_unicode_text(&mut content, max_bytes, &mut total_bytes)? {
                return Ok(None);
            }
            if content.representations().is_empty() {
                return Ok(None);
            }
            if let Some(source_app) = source_app {
                content = content.with_source_app(source_app);
            }
            Ok(Some(content))
        })
    }

    fn read_formats_limited(
        &mut self,
        formats: &BTreeSet<ClipboardFormat>,
        max_bytes: u64,
    ) -> Result<Option<ClipboardItem>> {
        let source_app = foreground_app();
        with_open_clipboard(|| unsafe {
            let mut content = ClipboardItem::for_platform(ClipboardPlatform::Windows);
            let mut total_bytes = 0;
            if !read_raw_representations(&mut content, Some(formats), max_bytes, &mut total_bytes)?
            {
                return Ok(None);
            }
            if format_requests_native_kind(formats, "CF_UNICODETEXT")
                && !read_unicode_text(&mut content, max_bytes, &mut total_bytes)?
            {
                return Ok(None);
            }
            if content.representations().is_empty() {
                return Ok(None);
            }
            Ok(Some(match source_app {
                Some(source_app) => content.with_source_app(source_app),
                None => content,
            }))
        })
    }

    fn metadata(&mut self) -> Result<ClipboardMetadata> {
        let ignored = with_open_clipboard(|| unsafe {
            if should_ignore_open_clipboard()? {
                log::info!("clipboard item ignored from native format markers");
                return Ok(true);
            }
            Ok(false)
        })?;
        if ignored {
            return Ok(ClipboardMetadata::ignored());
        }
        Ok(ClipboardMetadata::readable(foreground_app()))
    }

    fn write(&mut self, content: &ClipboardItem) -> Result<()> {
        with_open_clipboard(|| unsafe {
            if EmptyClipboard() == 0 {
                return Err(last_error("empty clipboard"));
            }
            let mut wrote_any = false;
            for representation in content.representations() {
                if stale_for_authored_semantic(content, representation.kind()) {
                    continue;
                }
                let Some(native_format) = native_format_id(representation) else {
                    continue;
                };
                set_hglobal_clipboard_data(native_format, representation.data())?;
                wrote_any = true;
            }
            if let Some(text) = content
                .text_semantic()
                .filter(|semantic| semantic.is_authored())
                .map(|semantic| semantic.value())
            {
                set_utf16_clipboard_data(CF_UNICODETEXT, text)?;
                wrote_any = true;
            }
            if let Some(html) = content
                .html_semantic()
                .filter(|semantic| semantic.is_authored())
                .map(|semantic| semantic.value())
            {
                let native_format = register_clipboard_format(WINDOWS_HTML_FORMAT)
                    .ok_or_else(|| anyhow::anyhow!("register Windows HTML format"))?;
                set_hglobal_clipboard_data(native_format, &cf_html::encode(html))?;
                wrote_any = true;
            }
            if let Some(rtf) = content
                .rtf_semantic()
                .filter(|semantic| semantic.is_authored())
                .map(|semantic| semantic.value())
            {
                let native_format = register_clipboard_format(WINDOWS_RTF_FORMAT)
                    .ok_or_else(|| anyhow::anyhow!("register Windows RTF format"))?;
                set_hglobal_clipboard_data(native_format, rtf.as_bytes())?;
                wrote_any = true;
            }
            if let Some(url) = content
                .url_semantic()
                .filter(|semantic| semantic.is_authored())
                .or_else(|| {
                    content
                        .file_url_semantic()
                        .filter(|semantic| semantic.is_authored())
                })
                .map(|semantic| semantic.value())
            {
                let native_format = register_clipboard_format(WINDOWS_URL_WIDE_FORMAT)
                    .ok_or_else(|| anyhow::anyhow!("register Windows URL format"))?;
                set_utf16_clipboard_data(native_format, url)?;
                wrote_any = true;
            }
            let marker = register_clipboard_format(SOURCE_MARKER_FORMAT)
                .ok_or_else(|| anyhow::anyhow!("register Windows source marker"))?;
            set_hglobal_clipboard_data(marker, &[1])?;
            wrote_any
                .then_some(())
                .ok_or_else(|| anyhow::anyhow!("clipboard item has no writable representations"))
        })
    }
}

unsafe fn should_ignore_open_clipboard() -> Result<bool> {
    let mut named_formats = Vec::new();
    let mut format = 0;
    loop {
        format = EnumClipboardFormats(format);
        if format == 0 {
            break;
        }
        if let Some(name) = registered_format_name(format) {
            if IGNORE_FORMATS.contains(&name.as_str()) {
                return Ok(true);
            }
            named_formats.push((format, name));
        }
    }

    for (format, name) in named_formats {
        if (name == HISTORY_FORMAT || name == CLOUD_FORMAT) && clipboard_dword(format) == Some(0) {
            return Ok(true);
        }
    }
    Ok(false)
}

unsafe fn clipboard_dword(format: u32) -> Option<u32> {
    let handle = GetClipboardData(format);
    if handle.is_null() || GlobalSize(handle) < std::mem::size_of::<u32>() {
        return None;
    }
    let data = GlobalLock(handle).cast::<u32>();
    if data.is_null() {
        return None;
    }
    let value = data.read_unaligned();
    let _ = GlobalUnlock(handle);
    Some(value)
}

unsafe fn read_raw_representations(
    content: &mut ClipboardItem,
    formats: Option<&BTreeSet<ClipboardFormat>>,
    max_bytes: u64,
    total_bytes: &mut u128,
) -> Result<bool> {
    let mut format = 0;
    loop {
        format = EnumClipboardFormats(format);
        if format == 0 {
            break;
        }
        if format == CF_UNICODETEXT || !is_hglobal_format(format) {
            continue;
        }
        let registered_name = registered_format_name(format);
        let native_name = registered_name
            .clone()
            .unwrap_or_else(|| predefined_format_name(format));
        if formats.is_some_and(|formats| !format_requests_native_kind(formats, &native_name)) {
            continue;
        }
        let handle = GetClipboardData(format);
        if handle.is_null() {
            continue;
        }
        let size = GlobalSize(handle);
        if size == 0 {
            continue;
        }
        *total_bytes = total_bytes.saturating_add(size as u128);
        if max_bytes > 0 && *total_bytes > u128::from(max_bytes) {
            log::info!("clipboard item ignored while reading size_bytes>{max_bytes}");
            return Ok(false);
        }
        let data = GlobalLock(handle).cast::<u8>();
        if data.is_null() {
            continue;
        }
        let bytes = std::slice::from_raw_parts(data, size).to_vec();
        let _ = GlobalUnlock(handle);
        if let Some(name) = registered_name {
            content.set_native(NativeRepresentation::windows_registered(
                name.clone(),
                format,
                bytes.clone(),
            ));
            if name == WINDOWS_HTML_FORMAT {
                if let Some(html) = cf_html::decode(&bytes) {
                    content.set_derived_html(html, vec![name]);
                }
                continue;
            }
            if name == WINDOWS_URL_WIDE_FORMAT {
                let wide = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|unit| u16::from_le_bytes(*unit))
                    .take_while(|unit| *unit != 0)
                    .collect::<Vec<_>>();
                let url = String::from_utf16_lossy(&wide);
                content.set_derived_url(&url, vec![name.clone()]);
                if url.starts_with("file:") {
                    content.set_derived_file_url(url, vec![name]);
                }
                continue;
            }
            if name == WINDOWS_URL_FORMAT {
                let end = bytes
                    .iter()
                    .position(|byte| *byte == 0)
                    .unwrap_or(bytes.len());
                if let Ok(url) = std::str::from_utf8(&bytes[..end]) {
                    content.set_derived_url(url, vec![name.clone()]);
                    if url.starts_with("file:") {
                        content.set_derived_file_url(url, vec![name]);
                    }
                }
                continue;
            }
            if name == WINDOWS_RTF_FORMAT {
                if let Ok(rtf) = std::str::from_utf8(&bytes) {
                    content.set_derived_rtf(
                        rtf.trim_end_matches('\0'),
                        vec![WINDOWS_RTF_FORMAT.into()],
                    );
                }
                continue;
            }
        } else {
            content.set_native(windows_predefined_representation(format, bytes));
        }
    }
    Ok(true)
}

unsafe fn read_unicode_text(
    content: &mut ClipboardItem,
    max_bytes: u64,
    total_bytes: &mut u128,
) -> Result<bool> {
    if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
        return Ok(true);
    }
    let handle = GetClipboardData(CF_UNICODETEXT);
    if handle.is_null() {
        return Err(last_error("read CF_UNICODETEXT clipboard handle"));
    }
    let size = GlobalSize(handle);
    if size < std::mem::size_of::<u16>() {
        return Ok(true);
    }
    *total_bytes = total_bytes.saturating_add(size as u128);
    if max_bytes > 0 && *total_bytes > u128::from(max_bytes) {
        log::info!("clipboard item ignored while reading size_bytes>{max_bytes}");
        return Ok(false);
    }
    let data = GlobalLock(handle);
    if data.is_null() {
        return Err(last_error("lock CF_UNICODETEXT clipboard data"));
    }
    let units = std::slice::from_raw_parts(data.cast::<u16>(), size / std::mem::size_of::<u16>());
    let bytes = std::slice::from_raw_parts(data.cast::<u8>(), size).to_vec();
    content.set_native(NativeRepresentation::windows_predefined(
        "CF_UNICODETEXT",
        CF_UNICODETEXT,
        bytes,
    ));
    content.set_derived_text(
        decode_nul_terminated_utf16(units),
        vec!["CF_UNICODETEXT".into()],
    );
    let _ = GlobalUnlock(handle);
    Ok(true)
}

fn is_hglobal_format(format: u32) -> bool {
    format >= REGISTERED_FORMAT_MIN || matches!(format, CF_DIB | CF_HDROP | CF_LOCALE | CF_DIBV5)
}

fn windows_predefined_representation(format: u32, bytes: Vec<u8>) -> NativeRepresentation {
    NativeRepresentation::windows_predefined(predefined_format_name(format), format, bytes)
}

fn predefined_format_name(format: u32) -> String {
    match format {
        CF_DIB => "CF_DIB".into(),
        CF_HDROP => "CF_HDROP".into(),
        CF_LOCALE => "CF_LOCALE".into(),
        CF_DIBV5 => "CF_DIBV5".into(),
        CF_UNICODETEXT => "CF_UNICODETEXT".into(),
        value => format!("CF_{value}"),
    }
}

unsafe fn registered_format_name(format: u32) -> Option<String> {
    if format < REGISTERED_FORMAT_MIN {
        return None;
    }
    let mut name = vec![0u16; 256];
    let len = GetClipboardFormatNameW(format, name.as_mut_ptr(), name.len() as i32);
    (len > 0).then(|| String::from_utf16_lossy(&name[..len as usize]))
}

unsafe fn native_format_id(representation: &NativeRepresentation) -> Option<u32> {
    if representation
        .flags()
        .contains(&NativeFormatFlag::Registered)
    {
        register_clipboard_format(representation.kind())
    } else {
        representation.id()
    }
}

unsafe fn register_clipboard_format(name: &str) -> Option<u32> {
    let wide = name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let id = RegisterClipboardFormatW(wide.as_ptr());
    (id != 0).then_some(id)
}

unsafe fn set_hglobal_clipboard_data(format: u32, bytes: &[u8]) -> Result<()> {
    let allocation = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
    if allocation.is_null() {
        return Err(last_error("allocate clipboard representation"));
    }
    let destination = GlobalLock(allocation).cast::<u8>();
    if destination.is_null() {
        let _ = GlobalFree(allocation);
        return Err(last_error("lock clipboard representation allocation"));
    }
    ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len());
    let _ = GlobalUnlock(allocation);
    if SetClipboardData(format, allocation).is_null() {
        let error = last_error("set clipboard representation");
        let _ = GlobalFree(allocation);
        return Err(error);
    }
    Ok(())
}

unsafe fn set_utf16_clipboard_data(format: u32, value: &str) -> Result<()> {
    let encoded = encode_nul_terminated_utf16(value);
    let bytes = std::slice::from_raw_parts(
        encoded.as_ptr().cast::<u8>(),
        encoded.len() * std::mem::size_of::<u16>(),
    );
    set_hglobal_clipboard_data(format, bytes)
}

fn stale_for_authored_semantic(content: &ClipboardItem, kind: &str) -> bool {
    match kind {
        "CF_UNICODETEXT" => content
            .text_semantic()
            .is_some_and(|semantic| semantic.is_authored()),
        WINDOWS_HTML_FORMAT => content
            .html_semantic()
            .is_some_and(|semantic| semantic.is_authored()),
        WINDOWS_RTF_FORMAT => content
            .rtf_semantic()
            .is_some_and(|semantic| semantic.is_authored()),
        WINDOWS_URL_FORMAT | WINDOWS_URL_WIDE_FORMAT => content
            .url_semantic()
            .or_else(|| content.file_url_semantic())
            .is_some_and(|semantic| semantic.is_authored()),
        _ => false,
    }
}

fn with_open_clipboard<T>(operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let mut opened = false;
    for attempt in 0..OPEN_ATTEMPTS {
        if unsafe { OpenClipboard(HWND::default()) } != 0 {
            opened = true;
            break;
        }
        if attempt + 1 < OPEN_ATTEMPTS {
            thread::sleep(OPEN_RETRY_DELAY);
        }
    }
    if !opened {
        return Err(last_error("open clipboard after retries"));
    }

    struct ClipboardGuard;
    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }
    let _guard = ClipboardGuard;
    operation()
}

fn decode_nul_terminated_utf16(data: &[u16]) -> String {
    let len = data
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(data.len());
    String::from_utf16_lossy(&data[..len])
}

fn encode_nul_terminated_utf16(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn foreground_app() -> Option<ClipboardSourceApp> {
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return None;
    }
    let mut process_id = 0;
    unsafe { GetWindowThreadProcessId(window, &mut process_id) };
    if process_id == 0 {
        return None;
    }

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return None;
    }
    struct ProcessGuard(HANDLE);
    impl Drop for ProcessGuard {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    let _guard = ProcessGuard(process);

    let mut path = vec![0u16; 32_768];
    let mut len = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut len) } == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&path[..len as usize]);
    source_app_from_executable(Path::new(&path))
}

fn source_app_from_executable(path: &Path) -> Option<ClipboardSourceApp> {
    let executable = path.file_name()?.to_str()?.to_string();
    let name = path
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_string);
    Some(ClipboardSourceApp::new(Some(executable), name))
}

fn last_error(action: &str) -> anyhow::Error {
    anyhow::anyhow!("{action}: {}", std::io::Error::last_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_round_trip_preserves_non_bmp_text() {
        let encoded = encode_nul_terminated_utf16("hello 📋");
        assert_eq!(encoded.last(), Some(&0));
        let decoded = decode_nul_terminated_utf16(&encoded);
        assert_eq!(decoded, "hello 📋");
    }

    #[test]
    fn utf16_decode_is_bounded_when_nul_is_missing() {
        assert_eq!(
            decode_nul_terminated_utf16(&[b'o' as u16, b'k' as u16]),
            "ok"
        );
    }

    #[test]
    fn executable_metadata_matches_windows_app_filters() {
        let source = source_app_from_executable(Path::new(r"C:\Program Files\Browser\browser.exe"))
            .expect("source metadata");
        assert_eq!(source.bundle_id.as_deref(), Some("browser.exe"));
        assert_eq!(source.name.as_deref(), Some("browser"));
        assert!(source.matches_any(&["BROWSER.EXE".to_string()]));
        assert!(source.matches_any(&["Browser".to_string()]));
    }
}
