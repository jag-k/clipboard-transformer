use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use ashpd::desktop::notification::{Button, Notification, NotificationProxy, Priority};
use futures_util::StreamExt;
use uuid::Uuid;

use crate::{
    log_status_notification, log_transform_notification, NotificationBackend, StartupNotification,
    TransformNotification,
};
use crate::{ActionSink, NotificationAction};
use ct_i18n::human_duration;

const ACTION_UNDO: &str = "undo";
const ACTION_EDIT_RULE: &str = "edit-rule";
const ACTION_DISABLE_RULE: &str = "disable-rule";
const ACTION_RELOAD_CONFIG: &str = "reload-config";
const ACTION_OPEN_CONFIG: &str = "open-config";
const ACTION_OPEN_SUPPORT: &str = "app.open-support";
const STARTUP_FAILURE_ID: &str = "clipboard-transformer-linux-startup-failed";

#[derive(Clone, Debug, Default)]
struct NotificationContext {
    transform_id: Option<Uuid>,
    rule_id: Option<String>,
    disable_seconds: Option<u64>,
}

pub struct LinuxNotificationBackend {
    proxy: NotificationProxy,
    contexts: Arc<Mutex<HashMap<String, NotificationContext>>>,
}

impl LinuxNotificationBackend {
    pub fn new(commands: ActionSink) -> Result<Self> {
        let proxy = futures_lite::future::block_on(NotificationProxy::new())
            .context("connect to XDG Desktop Portal notification interface")?;
        if proxy.version() == 0 {
            anyhow::bail!("XDG Desktop Portal notification interface reported version zero");
        }

        let contexts = Arc::new(Mutex::new(HashMap::new()));
        start_action_listener(commands, Arc::clone(&contexts))?;
        log::info!(
            "native Linux portal notification backend initialized version={}",
            proxy.version()
        );
        Ok(Self { proxy, contexts })
    }

    fn remember(&self, id: &str, context: NotificationContext) {
        if let Ok(mut contexts) = self.contexts.lock() {
            contexts.insert(id.to_string(), context);
        }
    }

    fn add(&self, id: &str, notification: Notification) -> Result<()> {
        futures_lite::future::block_on(self.proxy.add_notification(id, notification))
            .with_context(|| format!("deliver Linux portal notification {id}"))
    }
}

pub fn present_startup_failure(body: &str) -> Result<()> {
    let proxy = futures_lite::future::block_on(NotificationProxy::new())
        .context("connect to XDG Desktop Portal notification interface")?;
    let notification =
        Notification::new("Clipboard Transformer cannot run in this desktop session")
            .body(body)
            .priority(Priority::High)
            .default_action(ACTION_OPEN_SUPPORT)
            .button(Button::new("Open setup instructions", ACTION_OPEN_SUPPORT));
    futures_lite::future::block_on(proxy.add_notification(STARTUP_FAILURE_ID, notification))
        .context("present Linux desktop startup failure")
}

impl NotificationBackend for LinuxNotificationBackend {
    fn deliver_startup(&mut self, notification: StartupNotification) -> Result<()> {
        log_status_notification("schedule", &notification);
        let mut portal = Notification::new(&notification.title)
            .body(notification.body.as_str())
            .priority(Priority::High);
        if notification.reload_request_path.is_some() {
            portal = portal.button(Button::new("Reload", ACTION_RELOAD_CONFIG));
        }
        if notification.edit_target.is_some() {
            portal = portal
                .default_action(ACTION_OPEN_CONFIG)
                .button(Button::new("Open config", ACTION_OPEN_CONFIG));
        }
        self.remember(
            &notification.notification_id,
            NotificationContext::default(),
        );
        self.add(&notification.notification_id, portal)
    }

    fn deliver_transform(&mut self, notification: TransformNotification) -> Result<()> {
        log_transform_notification("schedule", &notification);
        let mut portal = Notification::new(&notification.title)
            .body(notification.body.as_str())
            .priority(Priority::Normal)
            .button(Button::new("Undo", ACTION_UNDO));
        if notification.edit_target.is_some() {
            portal = portal
                .default_action(ACTION_EDIT_RULE)
                .button(Button::new("Edit rule", ACTION_EDIT_RULE));
        }
        if let (Some(_), Some(seconds)) = (
            notification.rule_id.as_deref(),
            notification.disable_for_seconds,
        ) {
            portal = portal.button(Button::new(
                &format!(
                    "Disable for {}",
                    human_duration(std::time::Duration::from_secs(seconds))
                ),
                ACTION_DISABLE_RULE,
            ));
        }
        self.remember(
            &notification.notification_id,
            NotificationContext {
                transform_id: Some(notification.transform_id),
                rule_id: notification.rule_id,
                disable_seconds: notification.disable_for_seconds,
            },
        );
        self.add(&notification.notification_id, portal)
    }

    fn remove_delivered(&mut self, notification_id: &str) -> Result<()> {
        if let Ok(mut contexts) = self.contexts.lock() {
            contexts.remove(notification_id);
        }
        futures_lite::future::block_on(self.proxy.remove_notification(notification_id))
            .with_context(|| format!("remove Linux portal notification {notification_id}"))
    }
}

fn start_action_listener(
    commands: ActionSink,
    contexts: Arc<Mutex<HashMap<String, NotificationContext>>>,
) -> Result<()> {
    let (ready_sender, ready_receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("linux-portal-actions".into())
        .spawn(move || {
            futures_lite::future::block_on(async move {
                let proxy = match NotificationProxy::new().await {
                    Ok(proxy) => proxy,
                    Err(error) => {
                        let _ = ready_sender.send(Err(error.to_string()));
                        return;
                    }
                };
                let mut actions = match proxy.receive_action_invoked().await {
                    Ok(actions) => actions,
                    Err(error) => {
                        let _ = ready_sender.send(Err(error.to_string()));
                        return;
                    }
                };
                let _ = ready_sender.send(Ok(()));
                while let Some(action) = actions.next().await {
                    log::info!(
                        "notification action received notification_id={} action={}",
                        action.id(),
                        action.name()
                    );
                    let context = contexts
                        .lock()
                        .ok()
                        .and_then(|contexts| contexts.get(action.id()).cloned())
                        .unwrap_or_default();
                    let Some(command) = decode_action(action.name(), context) else {
                        log::info!(
                            "notification action missing context notification_id={} action={}",
                            action.id(),
                            action.name()
                        );
                        continue;
                    };
                    commands(command);
                }
            });
        })
        .context("start Linux portal notification action listener")?;

    ready_receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .context("wait for Linux portal notification action listener")?
        .map_err(anyhow::Error::msg)
}

fn decode_action(action: &str, context: NotificationContext) -> Option<NotificationAction> {
    match action {
        ACTION_UNDO => context
            .transform_id
            .map(|transform_id| NotificationAction::Undo { transform_id }),
        ACTION_EDIT_RULE => Some(NotificationAction::EditRule {
            rule_id: context.rule_id,
        }),
        ACTION_DISABLE_RULE => context
            .rule_id
            .zip(context.disable_seconds)
            .map(|(rule_id, seconds)| NotificationAction::DisableRule { rule_id, seconds }),
        ACTION_RELOAD_CONFIG => Some(NotificationAction::ReloadConfig),
        ACTION_OPEN_CONFIG => Some(NotificationAction::OpenConfig),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_context_maps_to_portable_commands() {
        let transform_id = Uuid::new_v4();
        let context = NotificationContext {
            transform_id: Some(transform_id),
            rule_id: Some("rule".into()),
            disable_seconds: Some(60),
        };
        assert_eq!(
            decode_action(ACTION_UNDO, context.clone()),
            Some(NotificationAction::Undo { transform_id })
        );
        assert_eq!(
            decode_action(ACTION_DISABLE_RULE, context.clone()),
            Some(NotificationAction::DisableRule {
                rule_id: "rule".into(),
                seconds: 60,
            })
        );
        assert_eq!(
            decode_action(ACTION_EDIT_RULE, context),
            Some(NotificationAction::EditRule {
                rule_id: Some("rule".into()),
            })
        );
    }
}
