//! Native tray backends, selected by `cfg(target_os)`.
//!
//! A backend receives its menu source once, at construction, and offers no way to
//! replace it. Telling a tray to re-read its menu can close a menu the user has
//! open, so the menu must only ever be built in response to the user opening it:
//! macOS in `menuNeedsUpdate:`, Linux in `ksni::Tray::menu`, Windows in
//! `show_menu`. Anything the user cannot report — currently only the system
//! light/dark preference — goes through `poll_chrome`, which never touches the
//! menu.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::LinuxTray;
#[cfg(target_os = "macos")]
pub use macos::MacosTray;
#[cfg(target_os = "windows")]
pub use windows::WindowsTray;

/// The tray backend for the host platform.
#[cfg(target_os = "macos")]
pub type Tray = MacosTray;
#[cfg(target_os = "windows")]
pub type Tray = WindowsTray;
#[cfg(target_os = "linux")]
pub type Tray = LinuxTray;
