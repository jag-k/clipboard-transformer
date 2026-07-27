#[cfg(feature = "desktop")]
pub mod autostart;
pub mod capabilities;
pub mod download;
pub mod environment;
#[cfg(feature = "desktop")]
pub mod host;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(feature = "desktop")]
pub mod open;
#[cfg(feature = "desktop")]
pub mod tray;
#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(feature = "desktop")]
pub use host::{
    bootstrap_host_environment, deliver_startup_failure, handle_early_host_command, instance_guard,
    notification_backend, present_runtime_failure, register_host_activation,
    verify_desktop_session, HostActivation, HostInstanceGuard,
};

pub use capabilities::{PlatformCapabilities, SupportLevel};
