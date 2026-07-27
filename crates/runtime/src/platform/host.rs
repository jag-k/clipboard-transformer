//! Per-OS host wiring, so application code contains no `cfg` branches.
//!
//! Each function here is the whole platform decision for one concern. The rule
//! is the same one the native crates follow: the choice of platform belongs
//! behind a function, not at the call site.

use std::path::Path;

use anyhow::Result;

use crate::logging;
use ct_notifications::{NotificationBackend, StartupNotification};

/// Holds the single-instance claim for the process lifetime.
pub struct HostInstanceGuard {
    #[cfg(target_os = "macos")]
    _inner: super::macos::instance::InstanceGuard,
    #[cfg(target_os = "linux")]
    _inner: super::linux::instance::InstanceGuard,
    #[cfg(target_os = "windows")]
    _inner: super::windows::instance::InstanceGuard,
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    _inner: (),
}

/// Claims single-instance ownership, replacing a previous instance where the
/// platform supports it.
///
/// `pid_path` is unused on Windows, which claims a named object instead of a pid
/// file.
pub fn instance_guard(pid_path: &Path) -> Result<HostInstanceGuard> {
    #[cfg(target_os = "macos")]
    let _inner = super::macos::instance::InstanceGuard::restart_previous(pid_path.to_path_buf())?;
    #[cfg(target_os = "linux")]
    let _inner = super::linux::instance::InstanceGuard::restart_previous(pid_path.to_path_buf())?;
    #[cfg(target_os = "windows")]
    let _inner = {
        let _ = pid_path;
        super::windows::instance::InstanceGuard::claim()?
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let _inner = {
        let _ = pid_path;
    };
    Ok(HostInstanceGuard { _inner })
}

/// Samples and logs the environment a GUI launch does not inherit.
///
/// Logging happens here because the report's fields differ per platform; a
/// caller would need a `cfg` branch just to format it.
pub fn bootstrap_host_environment() {
    #[cfg(unix)]
    {
        let report = super::environment::bootstrap_gui_environment();
        logging::event(format!(
            "GUI login-shell environment bootstrap shell={} imported={}",
            report
                .shell
                .as_deref()
                .map_or_else(|| "unavailable".into(), |shell| shell.display().to_string()),
            report.imported_count
        ));
        if let Some(warning) = report.warning {
            logging::event(format!("GUI environment bootstrap warning: {warning}"));
        }
    }
}

/// Fails before any backend starts when the session cannot support the desktop
/// runtime. Only Linux has a session that can be missing these capabilities.
pub fn verify_desktop_session() -> Result<()> {
    #[cfg(target_os = "linux")]
    return super::linux::verify_session();
    #[cfg(not(target_os = "linux"))]
    Ok(())
}

/// The notification backend for this host, including any platform fallback.
///
/// macOS aborts the process when `UNUserNotificationCenter` is used outside an
/// `.app` bundle, so a bundle-less launch degrades to the headless backend
/// instead. That decision is platform knowledge, so it lives here.
pub fn notification_backend(
    actions: ct_notifications::ActionSink,
    app_user_model_id: &str,
    disable_for_seconds: u64,
) -> Result<Box<dyn NotificationBackend>> {
    #[cfg(target_os = "macos")]
    {
        if !super::macos::launch::launched_as_app() {
            logging::event("using headless notification backend outside app bundle");
            return Ok(Box::new(ct_notifications::HeadlessNotificationBackend));
        }
    }
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        logging::event("using native notification backend");
        ct_notifications::native::backend(actions, app_user_model_id, disable_for_seconds)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (actions, app_user_model_id, disable_for_seconds);
        logging::event("using headless notification backend on an unsupported platform");
        Ok(Box::new(ct_notifications::HeadlessNotificationBackend))
    }
}

/// Tells the user a required backend failed, through whatever channel exists
/// before the runtime is up. A no-op where there is none.
pub fn present_runtime_failure(error: &anyhow::Error) {
    #[cfg(target_os = "linux")]
    super::linux::present_runtime_failure(error);
    #[cfg(not(target_os = "linux"))]
    logging::event(format!("desktop backend failure: {error:#}"));
}

/// Delivers a startup failure notification without a running agent.
///
/// Nothing can act on an action here, so the sink is empty: the process is about
/// to exit.
pub fn deliver_startup_failure(notification: StartupNotification, disable_for_seconds: u64) {
    #[cfg(target_os = "macos")]
    {
        // Outside an .app bundle the native backend aborts the process; the
        // error already reached the log and the stderr mirror.
        if !super::macos::launch::launched_as_app() {
            return;
        }
        if let Ok(mut backend) = ct_notifications::native::MacosNotificationBackend::new(
            Box::new(|_| {}),
            disable_for_seconds,
        ) {
            let _ = backend
                .deliver_startup_and_wait(notification, std::time::Duration::from_millis(750));
        }
    }
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        if let Ok(mut backend) = ct_notifications::native::backend(
            Box::new(|_| {}),
            crate::APP_USER_MODEL_ID,
            disable_for_seconds,
        ) {
            let _ = backend.deliver_startup(notification);
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (notification, disable_for_seconds);
    }
}

/// Handles hidden argv commands the host re-invokes itself with.
///
/// Returns `true` when the process handled one and must exit without starting a
/// desktop instance. Kept here because which commands exist is platform
/// knowledge: the Unix environment bootstrap re-runs this executable, and Linux
/// D-Bus activation starts it with `--gapplication-service`.
pub fn handle_early_host_command() -> bool {
    #[cfg(unix)]
    {
        let mut args = std::env::args().skip(1);
        let first = args.next();
        if first.as_deref() == Some("__dump-environment") {
            let marker = args.next().unwrap_or_default();
            if let Err(error) = super::environment::dump_current_environment(&marker) {
                eprintln!("dump environment failed: {error}");
                std::process::exit(1);
            }
            return true;
        }
        #[cfg(target_os = "linux")]
        if first.as_deref() == Some("--gapplication-service") {
            if let Err(error) = super::linux::activation::run_service() {
                eprintln!("D-Bus activation service failed: {error:#}");
                std::process::exit(1);
            }
            return true;
        }
    }
    false
}

/// Registers whatever the platform needs before a desktop instance starts, and
/// returns a guard that keeps the registration alive.
pub struct HostActivation {
    #[cfg(target_os = "windows")]
    _class_object: super::windows::activation::ClassObjectRegistration,
    #[cfg(not(target_os = "windows"))]
    _inner: (),
}

/// Claims the toast activator and refreshes the launcher entry on Windows; a
/// no-op elsewhere.
pub fn register_host_activation() -> Result<HostActivation> {
    #[cfg(target_os = "windows")]
    {
        use super::windows::{activation, registration};

        let _class_object = activation::ClassObjectRegistration::register()?;
        // Process-wide identity used by every toast, and the Start Menu entry
        // that carries the activator CLSID.
        registration::set_process_app_user_model_id()?;
        registration::ensure_desktop_registration(&std::env::current_exe()?)?;
        Ok(HostActivation { _class_object })
    }
    #[cfg(not(target_os = "windows"))]
    Ok(HostActivation { _inner: () })
}
