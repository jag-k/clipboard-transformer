//! Native clipboard backends, selected by `cfg(target_os)`.
//!
//! Behind the `native` feature so a host that supplies its own clipboard access
//! — a browser extension, for example — can depend on the item model, formats,
//! and codecs without pulling in any platform crate.

use anyhow::Result;

use crate::ClipboardBackend;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{
    probe_clipboard_backend, LinuxClipboardBackend, LinuxClipboardBackendKind, X11ClipboardBackend,
};
#[cfg(target_os = "macos")]
pub use macos::MacosClipboardBackend;
#[cfg(target_os = "linux")]
pub use wayland::WaylandClipboardBackend;
#[cfg(target_os = "windows")]
pub use windows::WindowsClipboardBackend;

/// The clipboard backend for the host platform.
///
/// Hosts call this instead of naming a platform type, which is what keeps
/// `cfg(target_os)` branches out of application code.
pub fn backend() -> Result<Box<dyn ClipboardBackend>> {
    #[cfg(target_os = "macos")]
    return Ok(Box::new(MacosClipboardBackend::new()?));
    #[cfg(target_os = "windows")]
    return Ok(Box::new(WindowsClipboardBackend::new()?));
    #[cfg(target_os = "linux")]
    return Ok(Box::new(LinuxClipboardBackend::new()?));
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    anyhow::bail!(
        "clipboard access is not implemented for {}",
        std::env::consts::OS
    );
}
