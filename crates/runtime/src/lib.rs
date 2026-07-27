#[cfg(feature = "desktop")]
pub mod app;
pub mod config;
pub mod logging;
pub mod platform;
pub mod plugins;
pub mod rules;
pub mod state;

/// Windows AppUserModelID and general application identity, shared by the toast
/// notifier, the COM activator, and the Start Menu shortcut.
pub const APP_USER_MODEL_ID: &str = "dev.jag-k.clipboard-transformer";

pub use config::{ConfigDocument, ConfigPaths};
