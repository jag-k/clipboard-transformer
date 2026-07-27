#[cfg(feature = "desktop")]
pub mod activation;
pub mod diagnostics;
#[cfg(feature = "desktop")]
#[path = "../unix_instance.rs"]
pub mod instance;

use anyhow::Result;

/// Fatal capability preflight: refuses to start degraded.
pub fn verify_session() -> Result<()> {
    let diagnostics = diagnostics::LinuxSessionDiagnostics::probe();
    crate::logging::event(format!(
        "Linux session probe session_type={} desktop={} clipboard_backend={} session_bus={} status_notifier_host={} notifications={} ready={}",
        diagnostics.session_type,
        diagnostics.desktop.as_deref().unwrap_or("unknown"),
        diagnostics
            .clipboard_backend
            .map_or_else(|| "none".to_string(), |backend| backend.to_string()),
        diagnostics.session_bus,
        diagnostics.status_notifier_host,
        diagnostics.notifications,
        diagnostics.desktop_runtime_ready,
    ));
    for blocker in &diagnostics.blockers {
        crate::logging::event(format!("Linux desktop blocker: {blocker}"));
    }
    if diagnostics.desktop_runtime_ready {
        return Ok(());
    }

    let blockers = diagnostics
        .blockers
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!(
        "Required desktop capabilities are unavailable: {blockers}. Clipboard Transformer will exit instead of running in a degraded mode. Setup instructions: {}",
        diagnostics::SUPPORT_URL
    );
    if diagnostics.notifications == diagnostics::Availability::Available {
        if let Err(error) = ct_notifications::native::present_startup_failure(&body) {
            crate::logging::event(format!(
                "present Linux startup failure notification failed: {error:#}"
            ));
        }
    }
    anyhow::bail!("Clipboard Transformer cannot run in this desktop session: {body}")
}

/// Tells the user a required backend failed, via the portal.
pub fn present_runtime_failure(error: &anyhow::Error) {
    let body = format!(
        "A required Linux desktop backend failed to initialize: {error}. Clipboard Transformer will exit instead of running in a degraded mode. Setup instructions: {}",
        diagnostics::SUPPORT_URL
    );
    crate::logging::event(format!("Linux desktop backend failure: {error:#}"));
    if let Err(notification_error) = ct_notifications::native::present_startup_failure(&body) {
        crate::logging::event(format!(
            "present Linux backend failure notification failed: {notification_error:#}"
        ));
    }
}
