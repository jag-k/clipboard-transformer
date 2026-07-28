use anyhow::Result;
use uuid::Uuid;

mod backends;
#[cfg(feature = "native")]
pub mod native;

pub(crate) use backends::{log_status_notification, log_transform_notification};
pub use backends::{HeadlessNotificationBackend, NoopNotificationBackend};

/// What the user chose on a delivered notification.
///
/// A concrete type rather than the host's own command enum, because `objc2`'s
/// `define_class!` cannot be generic (Objective-C classes are not), and the
/// macOS backend stores its callback in class ivars. Hosts convert this into
/// their own command at the [`ActionSink`] they supply, so no adapter thread and
/// no second channel appear in between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationAction {
    Undo { transform_id: Uuid },
    EditRule { rule_id: Option<String> },
    DisableRule { rule_id: String, seconds: u64 },
    ReloadConfig,
    OpenConfig,
}

/// Where backends deliver chosen actions.
///
/// A closure rather than a `Sender`, so the host can send its own command type
/// into the one channel it already drains, keeping tray and notification
/// actions ordered relative to each other.
pub type ActionSink = Box<dyn Fn(NotificationAction) + Send>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformNotification {
    pub notification_id: String,
    pub transform_id: Uuid,
    pub rule_id: Option<String>,
    pub title: String,
    pub body: String,
    pub disable_for_seconds: Option<u64>,
    pub edit_target: Option<EditTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupNotification {
    pub notification_id: String,
    pub title: String,
    pub body: String,
    pub edit_target: Option<EditTarget>,
    pub reload_request_path: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditTarget {
    pub path: String,
    pub line: Option<usize>,
}

/// Delivery and removal contract.
///
/// Action events are intentionally absent: each platform delivers them through
/// a different mechanism (a main-thread delegate on macOS, a dedicated D-Bus
/// thread on Linux, COM activation on Windows), so backends publish the host's
/// own command type over a channel the host owns.
pub trait NotificationBackend {
    fn configure_disable_for(&mut self, _seconds: u64) -> Result<()> {
        Ok(())
    }

    fn deliver_startup(&mut self, notification: StartupNotification) -> Result<()>;
    fn deliver_transform(&mut self, notification: TransformNotification) -> Result<()>;
    fn remove_delivered(&mut self, notification_id: &str) -> Result<()>;
}

impl<T> NotificationBackend for Box<T>
where
    T: NotificationBackend + ?Sized,
{
    fn configure_disable_for(&mut self, seconds: u64) -> Result<()> {
        (**self).configure_disable_for(seconds)
    }

    fn deliver_startup(&mut self, notification: StartupNotification) -> Result<()> {
        (**self).deliver_startup(notification)
    }

    fn deliver_transform(&mut self, notification: TransformNotification) -> Result<()> {
        (**self).deliver_transform(notification)
    }

    fn remove_delivered(&mut self, notification_id: &str) -> Result<()> {
        (**self).remove_delivered(notification_id)
    }
}
