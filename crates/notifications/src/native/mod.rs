//! Native notification backends, selected by `cfg(target_os)`.
//!
//! Action delivery differs per platform and none of it needs an event loop of
//! its own: macOS uses a main-thread `UNUserNotificationCenter` delegate, Linux
//! a dedicated D-Bus listener thread, and Windows COM toast activation that the
//! host registers before the loop starts. macOS does require that *someone*
//! pumps the main run loop for delegate callbacks to arrive — a precondition of
//! the environment, not a dependency of this crate.

use anyhow::Result;

use crate::{ActionSink, NotificationBackend};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{present_startup_failure, LinuxNotificationBackend};
#[cfg(target_os = "macos")]
pub use macos::MacosNotificationBackend;
#[cfg(target_os = "windows")]
pub use windows::WindowsNotificationBackend;

/// The notification backend for the host platform.
///
/// `disable_for_seconds` is only meaningful where the platform registers action
/// categories up front (macOS); elsewhere it is accepted and ignored so hosts
/// need no `cfg` branch.
pub fn backend(
    actions: ActionSink,
    app_user_model_id: &str,
    #[cfg_attr(
        not(target_os = "macos"),
        allow(
            unused_variables,
            reason = "only macOS pre-registers action categories"
        )
    )]
    disable_for_seconds: u64,
) -> Result<Box<dyn NotificationBackend>> {
    #[cfg(not(target_os = "windows"))]
    let _ = app_user_model_id;
    #[cfg(target_os = "macos")]
    return Ok(Box::new(MacosNotificationBackend::new(
        actions,
        disable_for_seconds,
    )?));
    #[cfg(target_os = "windows")]
    return Ok(Box::new(WindowsNotificationBackend::new(
        actions,
        app_user_model_id.to_string(),
    )?));
    #[cfg(target_os = "linux")]
    return Ok(Box::new(LinuxNotificationBackend::new(actions)?));
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = actions;
        anyhow::bail!(
            "native notifications are not implemented for {}",
            std::env::consts::OS
        )
    }
}
