use std::fmt;

use serde::Serialize;

use ct_clipboard::native::{probe_clipboard_backend, LinuxClipboardBackendKind};

pub const SUPPORT_URL: &str =
    "https://github.com/jag-k/clipboard-transformer/blob/main/docs/linux.md";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Availability {
    Available,
    Unavailable,
}

impl fmt::Display for Availability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Available => formatter.write_str("available"),
            Self::Unavailable => formatter.write_str("unavailable"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinuxSessionType {
    Wayland,
    X11,
    Unknown,
}

impl fmt::Display for LinuxSessionType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wayland => formatter.write_str("wayland"),
            Self::X11 => formatter.write_str("x11"),
            Self::Unknown => formatter.write_str("unknown"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinuxStartupFailureCode {
    ClipboardObservationUnavailable,
    SessionBusUnavailable,
    StatusNotifierHostUnavailable,
    NotificationsUnavailable,
}

impl fmt::Display for LinuxStartupFailureCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClipboardObservationUnavailable => {
                formatter.write_str("clipboard-observation-unavailable")
            }
            Self::SessionBusUnavailable => formatter.write_str("session-bus-unavailable"),
            Self::StatusNotifierHostUnavailable => {
                formatter.write_str("status-notifier-host-unavailable")
            }
            Self::NotificationsUnavailable => formatter.write_str("notifications-unavailable"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LinuxSessionDiagnostics {
    pub session_type: LinuxSessionType,
    pub desktop: Option<String>,
    pub session_bus: Availability,
    pub clipboard_observation: Availability,
    pub clipboard_backend: Option<LinuxClipboardBackendKind>,
    pub status_notifier_host: Availability,
    pub notifications: Availability,
    pub desktop_runtime_ready: bool,
    pub blockers: Vec<LinuxStartupFailureCode>,
}

impl LinuxSessionDiagnostics {
    /// Performs protocol and service discovery without registering a tray,
    /// reading clipboard contents, or creating application files.
    pub fn probe() -> Self {
        let session_type = detected_session_type();
        let desktop = non_empty_env("XDG_CURRENT_DESKTOP");
        let clipboard_backend = probe_clipboard_backend().ok().flatten();
        let (session_bus, status_notifier_host, notifications) = probe_session_bus();

        let mut blockers = Vec::new();
        if clipboard_backend.is_none() {
            blockers.push(LinuxStartupFailureCode::ClipboardObservationUnavailable);
        }
        if session_bus == Availability::Unavailable {
            blockers.push(LinuxStartupFailureCode::SessionBusUnavailable);
        } else {
            if status_notifier_host == Availability::Unavailable {
                blockers.push(LinuxStartupFailureCode::StatusNotifierHostUnavailable);
            }
            if notifications == Availability::Unavailable {
                blockers.push(LinuxStartupFailureCode::NotificationsUnavailable);
            }
        }

        Self {
            session_type,
            desktop,
            session_bus,
            clipboard_observation: if clipboard_backend.is_some() {
                Availability::Available
            } else {
                Availability::Unavailable
            },
            clipboard_backend,
            status_notifier_host,
            notifications,
            desktop_runtime_ready: blockers.is_empty(),
            blockers,
        }
    }
}

fn detected_session_type() -> LinuxSessionType {
    match non_empty_env("XDG_SESSION_TYPE")
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("wayland") => LinuxSessionType::Wayland,
        Some("x11") => LinuxSessionType::X11,
        _ if non_empty_env("WAYLAND_DISPLAY").is_some() => LinuxSessionType::Wayland,
        _ if non_empty_env("DISPLAY").is_some() => LinuxSessionType::X11,
        _ => LinuxSessionType::Unknown,
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn probe_session_bus() -> (Availability, Availability, Availability) {
    let Ok(connection) = zbus::blocking::Connection::session() else {
        return (
            Availability::Unavailable,
            Availability::Unavailable,
            Availability::Unavailable,
        );
    };
    (
        Availability::Available,
        availability(status_notifier_host_registered(&connection)),
        availability(notification_portal_available(&connection)),
    )
}

fn status_notifier_host_registered(connection: &zbus::blocking::Connection) -> bool {
    zbus::blocking::Proxy::new(
        connection,
        "org.kde.StatusNotifierWatcher",
        "/StatusNotifierWatcher",
        "org.kde.StatusNotifierWatcher",
    )
    .and_then(|proxy| proxy.get_property::<bool>("IsStatusNotifierHostRegistered"))
    .unwrap_or(false)
}

fn notification_portal_available(connection: &zbus::blocking::Connection) -> bool {
    zbus::blocking::Proxy::new(
        connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Notification",
    )
    .and_then(|proxy| proxy.get_property::<u32>("version"))
    .is_ok_and(|version| version > 0)
}

fn availability(value: bool) -> Availability {
    if value {
        Availability::Available
    } else {
        Availability::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_codes_are_stable_kebab_case() {
        assert_eq!(
            LinuxStartupFailureCode::ClipboardObservationUnavailable.to_string(),
            "clipboard-observation-unavailable"
        );
        assert_eq!(
            LinuxStartupFailureCode::StatusNotifierHostUnavailable.to_string(),
            "status-notifier-host-unavailable"
        );
    }
}
