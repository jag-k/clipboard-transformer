use anyhow::{Context, Result};
use windows::core::HSTRING;
use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

use crate::ActionSink;
use crate::{
    log_status_notification, log_transform_notification, NotificationBackend, StartupNotification,
    TransformNotification,
};
use ct_i18n::human_duration;

const NOTIFICATION_GROUP: &str = "clipboard-transformer";

/// Windows toast backend for the running desktop host.
pub struct WindowsNotificationBackend {
    app_user_model_id: String,
    delivered: std::collections::HashMap<String, ToastNotification>,
}

impl WindowsNotificationBackend {
    /// `app_user_model_id` is injected: it identifies the *application*, so this
    /// crate must not hardcode it. Setting it process-wide stays with the host,
    /// which also owns the COM activator registration that delivers actions.
    pub fn new(_commands: ActionSink, app_user_model_id: String) -> Result<Self> {
        Ok(Self {
            app_user_model_id,
            delivered: std::collections::HashMap::new(),
        })
    }

    fn show(
        &mut self,
        id: &str,
        title: &str,
        body: &str,
        body_argument: &str,
        actions: &[ToastAction],
    ) -> Result<()> {
        let xml = toast_xml(title, body, body_argument, actions);
        let document = XmlDocument::new().context("create Windows toast XML document")?;
        document
            .LoadXml(&HSTRING::from(xml))
            .context("load Windows toast XML")?;
        let toast = ToastNotification::CreateToastNotification(&document)
            .context("create Windows toast notification")?;
        let tag = toast_tag(id);
        toast
            .SetTag(&HSTRING::from(&tag))
            .context("set Windows toast tag")?;
        toast
            .SetGroup(&HSTRING::from(NOTIFICATION_GROUP))
            .context("set Windows toast group")?;

        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(
            self.app_user_model_id.as_str(),
        ))
        .context("create Windows toast notifier")?
        .Show(&toast)
        .context("show Windows toast")?;
        self.delivered.insert(tag, toast);
        Ok(())
    }
}

impl NotificationBackend for WindowsNotificationBackend {
    fn deliver_startup(&mut self, notification: StartupNotification) -> Result<()> {
        log_status_notification("schedule", &notification);
        let mut actions = Vec::new();
        if notification.reload_request_path.is_some() {
            actions.push(ToastAction::new("Reload", "reload"));
        }
        if notification.edit_target.is_some() {
            actions.push(ToastAction::new("Open config", "open-config"));
        }
        self.show(
            &notification.notification_id,
            &notification.title,
            &notification.body,
            if notification.edit_target.is_some() {
                "open-config"
            } else {
                ""
            },
            &actions,
        )
    }

    fn deliver_transform(&mut self, notification: TransformNotification) -> Result<()> {
        log_transform_notification("schedule", &notification);
        let mut actions = vec![ToastAction::new(
            "Undo",
            format!("undo:{}", notification.transform_id),
        )];
        if notification.edit_target.is_some() {
            actions.push(ToastAction::new(
                "Edit rule",
                format!(
                    "edit:{}",
                    notification.rule_id.as_deref().unwrap_or_default()
                ),
            ));
        }
        if let (Some(rule_id), Some(seconds)) = (
            notification.rule_id.as_deref(),
            notification.disable_for_seconds,
        ) {
            actions.push(ToastAction::new(
                format!(
                    "Disable for {}",
                    human_duration(std::time::Duration::from_secs(seconds))
                ),
                format!("disable:{seconds}:{rule_id}"),
            ));
        }
        self.show(
            &notification.notification_id,
            &notification.title,
            &notification.body,
            &format!(
                "edit:{}",
                notification.rule_id.as_deref().unwrap_or_default()
            ),
            &actions,
        )
    }

    fn remove_delivered(&mut self, notification_id: &str) -> Result<()> {
        let tag = toast_tag(notification_id);
        self.delivered.remove(&tag);
        ToastNotificationManager::History()
            .context("open Windows notification history")?
            .RemoveGroupedTagWithId(
                &HSTRING::from(tag),
                &HSTRING::from(NOTIFICATION_GROUP),
                &HSTRING::from(self.app_user_model_id.as_str()),
            )
            .context("remove delivered Windows notification")
    }
}

#[derive(Debug)]
struct ToastAction {
    label: String,
    argument: String,
}

impl ToastAction {
    fn new(label: impl Into<String>, argument: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            argument: argument.into(),
        }
    }
}

fn toast_xml(title: &str, body: &str, body_argument: &str, actions: &[ToastAction]) -> String {
    let mut xml = format!(
        r#"<toast launch="{}"><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual>"#,
        xml_escape(body_argument),
        xml_escape(title),
        xml_escape(body)
    );
    if !actions.is_empty() {
        xml.push_str("<actions>");
        for action in actions {
            xml.push_str(&format!(
                r#"<action activationType="foreground" content="{}" arguments="{}"/>"#,
                xml_escape(&action.label),
                xml_escape(&action.argument)
            ));
        }
        xml.push_str("</actions>");
    }
    xml.push_str("</toast>");
    xml
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_toast_content() {
        let xml = toast_xml(
            "A & B",
            "<body>",
            "edit:a&b",
            &[ToastAction::new("Use \"it\"", "edit:a&b")],
        );
        assert!(xml.contains(r#"launch="edit:a&amp;b""#));
        assert!(xml.contains("A &amp; B"));
        assert!(xml.contains("&lt;body&gt;"));
        assert!(xml.contains("Use &quot;it&quot;"));
        assert!(xml.contains("edit:a&amp;b"));
    }
}

/// Stable per-notification tag. WinRT identifies a toast by tag, and the ids we
/// hand out are longer than the platform allows, so hash them.
fn toast_tag(notification_id: &str) -> String {
    let hash = notification_id
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    format!("{hash:016x}")
}
