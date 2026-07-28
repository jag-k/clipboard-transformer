//! Test doubles and the diagnostic logging shared by every backend.

use anyhow::Result;

use crate::{NotificationBackend, StartupNotification, TransformNotification};

#[derive(Debug, Default)]
pub struct HeadlessNotificationBackend;

impl NotificationBackend for HeadlessNotificationBackend {
    fn deliver_startup(&mut self, notification: StartupNotification) -> Result<()> {
        log_status_notification("headless deliver", &notification);
        Ok(())
    }

    fn deliver_transform(&mut self, notification: TransformNotification) -> Result<()> {
        log_transform_notification("headless deliver", &notification);
        Ok(())
    }

    fn remove_delivered(&mut self, notification_id: &str) -> Result<()> {
        log::info!("notification headless remove notification_id={notification_id}");
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct NoopNotificationBackend {
    pub configured_disable_for: Vec<u64>,
    pub startups: Vec<StartupNotification>,
    pub delivered: Vec<TransformNotification>,
    pub removed: Vec<String>,
}

impl NotificationBackend for NoopNotificationBackend {
    fn configure_disable_for(&mut self, seconds: u64) -> Result<()> {
        self.configured_disable_for.push(seconds);
        Ok(())
    }

    fn deliver_startup(&mut self, notification: StartupNotification) -> Result<()> {
        log_status_notification("noop deliver", &notification);
        self.startups.push(notification);
        Ok(())
    }

    fn deliver_transform(&mut self, notification: TransformNotification) -> Result<()> {
        log_transform_notification("noop deliver", &notification);
        self.delivered.push(notification);
        Ok(())
    }

    fn remove_delivered(&mut self, notification_id: &str) -> Result<()> {
        log::info!("notification noop remove notification_id={notification_id}");
        self.removed.push(notification_id.to_string());
        Ok(())
    }
}

pub(crate) fn log_status_notification(action: &str, notification: &StartupNotification) {
    log::info!(
        "notification {action} kind=status notification_id={} title={} body={} actions={}",
        notification.notification_id,
        notification.title,
        notification.body,
        status_actions_summary(notification)
    );
}

pub(crate) fn log_transform_notification(action: &str, notification: &TransformNotification) {
    log::info!(
        "notification {action} kind=transform notification_id={} title={} body={} actions={}",
        notification.notification_id,
        notification.title,
        notification.body,
        transform_actions_summary(notification)
    );
}

fn transform_actions_summary(notification: &TransformNotification) -> String {
    let mut actions = vec!["Undo".to_string(), "Edit rule".to_string()];
    if let Some(seconds) = notification.disable_for_seconds {
        // Raw seconds on purpose: a log line does not need a localized
        // duration, and formatting one would drag a shared formatter into this
        // crate for no diagnostic benefit.
        actions.push(format!("Disable rule for {seconds}s"));
    }
    actions.join(", ")
}

fn status_actions_summary(notification: &StartupNotification) -> String {
    let mut actions = Vec::new();
    if notification.reload_request_path.is_some() {
        actions.push("Reload");
    }
    if notification.edit_target.is_some() {
        actions.push("Open config");
    }
    if actions.is_empty() {
        "none".to_string()
    } else {
        actions.join(", ")
    }
}
