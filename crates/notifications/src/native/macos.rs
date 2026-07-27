use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use block2::{Block, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSSet, NSString};
use objc2_foundation::{NSObject, NSObjectProtocol};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationAction,
    UNNotificationActionOptionNone, UNNotificationActionOptions, UNNotificationCategory,
    UNNotificationCategoryOptionNone, UNNotificationPresentationOptions, UNNotificationRequest,
    UNNotificationResponse, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use uuid::Uuid;

use crate::{
    log_status_notification, log_transform_notification, NotificationBackend, StartupNotification,
    TransformNotification,
};
use crate::{ActionSink, NotificationAction};
use ct_i18n::human_duration;

const CATEGORY_ID_WITH_DISABLE: &str = "clipboard-transformer.transform.with-disable";
const CATEGORY_ID_WITHOUT_DISABLE: &str = "clipboard-transformer.transform.without-disable";
const CATEGORY_ID_STATUS_WITH_RELOAD: &str = "clipboard-transformer.status.with-reload";
const ACTION_UNDO: &str = "clipboard-transformer.undo";
const ACTION_EDIT_RULE: &str = "clipboard-transformer.edit-rule";
const ACTION_DISABLE_RULE: &str = "clipboard-transformer.disable-rule";
const ACTION_RELOAD_CONFIG: &str = "clipboard-transformer.reload-config";
const ACTION_DEFAULT: &str = "com.apple.UNNotificationDefaultActionIdentifier";
const USER_INFO_EDIT_PATH: &str = "edit.path";
const USER_INFO_EDIT_LINE: &str = "edit.line";
const USER_INFO_RELOAD_REQUEST_PATH: &str = "reload.request_path";
const USER_INFO_TRANSFORM_ID: &str = "transform.id";
const USER_INFO_RULE_ID: &str = "rule.id";
const USER_INFO_DISABLE_SECONDS: &str = "disable.seconds";

type AppCommandSender = ActionSink;

pub struct MacosNotificationBackend {
    center: objc2::rc::Retained<UNUserNotificationCenter>,
    _delegate: Retained<NotificationDelegate>,
    disable_for_seconds: Option<u64>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = AppCommandSender]
    struct NotificationDelegate;

    unsafe impl NSObjectProtocol for NotificationDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            notification: &objc2_user_notifications::UNNotification,
            completion_handler: &Block<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            log::info!(
                "notification will present notification_id={}",
                notification.request().identifier()
            );
            completion_handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &Block<dyn Fn()>,
        ) {
            handle_notification_response(response, self.ivars());
            completion_handler.call(());
        }
    }
);

impl NotificationDelegate {
    fn new(commands: ActionSink) -> Retained<Self> {
        let this = Self::alloc().set_ivars(commands);
        unsafe { msg_send![super(this), init] }
    }
}

impl MacosNotificationBackend {
    pub fn new(command_sender: ActionSink, disable_for_seconds: u64) -> Result<Self> {
        log::info!("initializing native notification backend");
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate = NotificationDelegate::new(command_sender);
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        let mut backend = Self {
            center,
            _delegate: delegate,
            disable_for_seconds: (disable_for_seconds > 0).then_some(disable_for_seconds),
        };
        backend.log_notification_settings("before request");
        backend.request_authorization();
        backend.register_categories();
        log::info!("native notification backend initialized");
        Ok(backend)
    }

    fn log_notification_settings(&self, label: &'static str) {
        let completion = RcBlock::new(
            move |settings: std::ptr::NonNull<objc2_user_notifications::UNNotificationSettings>| {
                let settings = unsafe { settings.as_ref() };
                log::info!(
                        "notification settings {label}: authorization_status={} alert_setting={} sound_setting={} notification_center_setting={} lock_screen_setting={}",
                        settings.authorizationStatus().0,
                        settings.alertSetting().0,
                        settings.soundSetting().0,
                        settings.notificationCenterSetting().0,
                        settings.lockScreenSetting().0
                    );
                if settings.authorizationStatus()
                    == objc2_user_notifications::UNAuthorizationStatus::Denied
                {
                    log::info!(
                            "notification permission is denied; enable Clipboard Transformer in System Settings > Notifications",
                        );
                }
            },
        );
        self.center
            .getNotificationSettingsWithCompletionHandler(&completion);
    }

    fn request_authorization(&mut self) {
        log::info!("requesting notification authorization");
        let completion = RcBlock::new(|granted: objc2::runtime::Bool, error: *mut NSError| {
            let error_details = if error.is_null() {
                "none".to_string()
            } else {
                let error = unsafe { &*error };
                format!(
                    "domain={} code={} description={}",
                    error.domain(),
                    error.code(),
                    error.localizedDescription()
                )
            };
            log::info!(
                "notification authorization callback granted={} error={}",
                granted.as_bool(),
                error_details
            );
        });
        self.center
            .requestAuthorizationWithOptions_completionHandler(
                UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                &completion,
            );
    }

    fn register_categories(&mut self) {
        log::info!("registering notification categories");
        let undo = UNNotificationAction::actionWithIdentifier_title_options(
            &NSString::from_str(ACTION_UNDO),
            &NSString::from_str("Undo"),
            UNNotificationActionOptionNone,
        );
        let edit = UNNotificationAction::actionWithIdentifier_title_options(
            &NSString::from_str(ACTION_EDIT_RULE),
            &NSString::from_str("Edit rule"),
            UNNotificationActionOptions::Foreground,
        );
        let reload = UNNotificationAction::actionWithIdentifier_title_options(
            &NSString::from_str(ACTION_RELOAD_CONFIG),
            &NSString::from_str("Reload"),
            UNNotificationActionOptionNone,
        );
        let actions_without_disable = NSArray::from_retained_slice(&[undo, edit]);
        let status_actions = NSArray::from_retained_slice(&[reload]);
        let intents = NSArray::from_retained_slice(&[] as &[objc2::rc::Retained<NSString>]);
        let category_without_disable =
            UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                &NSString::from_str(CATEGORY_ID_WITHOUT_DISABLE),
                &actions_without_disable,
                &intents,
                UNNotificationCategoryOptionNone,
            );
        let category_status_with_reload =
            UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                &NSString::from_str(CATEGORY_ID_STATUS_WITH_RELOAD),
                &status_actions,
                &intents,
                UNNotificationCategoryOptionNone,
            );
        let mut categories = vec![category_without_disable, category_status_with_reload];
        if let Some(seconds) = self.disable_for_seconds {
            categories.push(transform_category_with_disable(seconds));
        }
        let categories = NSSet::from_retained_slice(&categories);
        self.center.setNotificationCategories(&categories);
        log::info!(
            "notification categories registered category_ids={}",
            registered_category_ids(self.disable_for_seconds).join(", ")
        );
    }

    pub fn deliver_startup_and_wait(
        &mut self,
        notification: StartupNotification,
        timeout: Duration,
    ) -> Result<()> {
        let (completion_sender, completion_receiver) = mpsc::sync_channel(1);
        self.schedule_startup(notification, Some(completion_sender))?;
        completion_receiver
            .recv_timeout(timeout)
            .map_err(|error| anyhow::anyhow!("wait for notification delivery completion: {error}"))?
            .map_err(anyhow::Error::msg)
    }

    fn schedule_startup(
        &mut self,
        notification: StartupNotification,
        completion_sender: Option<mpsc::SyncSender<std::result::Result<(), String>>>,
    ) -> Result<()> {
        log_status_notification("schedule", &notification);
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notification.title));
        content.setBody(&NSString::from_str(&notification.body));
        if let Some(user_info) = status_user_info(&notification) {
            let user_info = unsafe { user_info.cast_unchecked() };
            unsafe {
                content.setUserInfo(user_info);
            }
        }
        if notification.reload_request_path.is_some() {
            log::info!(
                "notification set category notification_id={} category_id={CATEGORY_ID_STATUS_WITH_RELOAD}",
                notification.notification_id
            );
            content.setCategoryIdentifier(&NSString::from_str(CATEGORY_ID_STATUS_WITH_RELOAD));
        }

        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&notification.notification_id),
            &content,
            None,
        );
        let notification_id = notification.notification_id.clone();
        let completion = RcBlock::new(move |error: *mut NSError| {
            let result = notification_completion_result(error);
            log_notification_completion("deliver status", &notification_id, error);
            if let Some(sender) = &completion_sender {
                let _ = sender.try_send(result);
            }
        });
        self.center
            .addNotificationRequest_withCompletionHandler(&request, Some(&completion));
        Ok(())
    }
}

impl NotificationBackend for MacosNotificationBackend {
    fn configure_disable_for(&mut self, seconds: u64) -> Result<()> {
        let disable_for_seconds = (seconds > 0).then_some(seconds);
        if self.disable_for_seconds != disable_for_seconds {
            self.disable_for_seconds = disable_for_seconds;
            self.register_categories();
        }
        Ok(())
    }

    fn deliver_startup(&mut self, notification: StartupNotification) -> Result<()> {
        self.schedule_startup(notification, None)
    }

    fn deliver_transform(&mut self, notification: TransformNotification) -> Result<()> {
        log_transform_notification("schedule", &notification);
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notification.title));
        content.setBody(&NSString::from_str(&notification.body));
        let user_info = transform_user_info(&notification);
        let user_info = unsafe { user_info.cast_unchecked() };
        unsafe {
            content.setUserInfo(user_info);
        }
        let category_id = transform_category_id(notification.disable_for_seconds);
        log::info!(
            "notification set category notification_id={} category_id={category_id}",
            notification.notification_id
        );
        content.setCategoryIdentifier(&NSString::from_str(category_id));

        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&notification.notification_id),
            &content,
            None,
        );
        let notification_id = notification.notification_id.clone();
        let completion = RcBlock::new(move |error: *mut NSError| {
            log_notification_completion("deliver transform", &notification_id, error);
        });
        self.center
            .addNotificationRequest_withCompletionHandler(&request, Some(&completion));
        Ok(())
    }

    fn remove_delivered(&mut self, notification_id: &str) -> Result<()> {
        log::info!(
            "notification remove kind=delivered-and-pending notification_id={notification_id}"
        );
        let identifiers = NSArray::from_retained_slice(&[NSString::from_str(notification_id)]);
        self.center
            .removeDeliveredNotificationsWithIdentifiers(&identifiers);
        self.center
            .removePendingNotificationRequestsWithIdentifiers(&identifiers);
        Ok(())
    }
}

fn log_notification_completion(action: &str, notification_id: &str, error: *mut NSError) {
    if error.is_null() {
        log::info!("notification {action} completed notification_id={notification_id} error=none");
        return;
    }

    let error = unsafe { &*error };
    log::info!(
            "notification {action} completed notification_id={notification_id} error=domain={} code={} description={}",
            error.domain(),
            error.code(),
            error.localizedDescription()
        );
}

fn notification_completion_result(error: *mut NSError) -> std::result::Result<(), String> {
    if error.is_null() {
        return Ok(());
    }

    let error = unsafe { &*error };
    Err(format!(
        "domain={} code={} description={}",
        error.domain(),
        error.code(),
        error.localizedDescription()
    ))
}

fn transform_category_id(disable_for_seconds: Option<u64>) -> &'static str {
    if disable_for_seconds.is_some() {
        CATEGORY_ID_WITH_DISABLE
    } else {
        CATEGORY_ID_WITHOUT_DISABLE
    }
}

fn registered_category_ids(disable_for_seconds: Option<u64>) -> Vec<&'static str> {
    let mut ids = vec![CATEGORY_ID_WITHOUT_DISABLE, CATEGORY_ID_STATUS_WITH_RELOAD];
    if disable_for_seconds.is_some() {
        ids.push(CATEGORY_ID_WITH_DISABLE);
    }
    ids
}

fn transform_category_with_disable(seconds: u64) -> Retained<UNNotificationCategory> {
    let undo = UNNotificationAction::actionWithIdentifier_title_options(
        &NSString::from_str(ACTION_UNDO),
        &NSString::from_str("Undo"),
        UNNotificationActionOptionNone,
    );
    let edit = UNNotificationAction::actionWithIdentifier_title_options(
        &NSString::from_str(ACTION_EDIT_RULE),
        &NSString::from_str("Edit rule"),
        UNNotificationActionOptions::Foreground,
    );
    let disable = UNNotificationAction::actionWithIdentifier_title_options(
        &NSString::from_str(ACTION_DISABLE_RULE),
        &NSString::from_str(&format!(
            "Disable rule for {}",
            human_duration(Duration::from_secs(seconds))
        )),
        UNNotificationActionOptionNone,
    );
    let actions = NSArray::from_retained_slice(&[undo, edit, disable]);
    let intents = NSArray::from_retained_slice(&[] as &[objc2::rc::Retained<NSString>]);
    UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &NSString::from_str(CATEGORY_ID_WITH_DISABLE),
        &actions,
        &intents,
        UNNotificationCategoryOptionNone,
    )
}

fn transform_user_info(
    notification: &TransformNotification,
) -> Retained<NSDictionary<NSString, NSString>> {
    let mut keys = vec![NSString::from_str(USER_INFO_TRANSFORM_ID)];
    let mut values = vec![NSString::from_str(&notification.transform_id.to_string())];
    if let Some(rule_id) = &notification.rule_id {
        keys.push(NSString::from_str(USER_INFO_RULE_ID));
        values.push(NSString::from_str(rule_id));
    }
    if let Some(seconds) = notification.disable_for_seconds {
        keys.push(NSString::from_str(USER_INFO_DISABLE_SECONDS));
        values.push(NSString::from_str(&seconds.to_string()));
    }
    if let Some(edit_target) = &notification.edit_target {
        keys.push(NSString::from_str(USER_INFO_EDIT_PATH));
        values.push(NSString::from_str(&edit_target.path));
        if let Some(line) = edit_target.line {
            keys.push(NSString::from_str(USER_INFO_EDIT_LINE));
            values.push(NSString::from_str(&line.to_string()));
        }
    }

    let key_refs = keys.iter().map(|key| &**key).collect::<Vec<_>>();
    NSDictionary::from_retained_objects(&key_refs, &values)
}

#[derive(Debug, Default)]
struct NotificationActionContext {
    transform_id: Option<Uuid>,
    rule_id: Option<String>,
    disable_seconds: Option<u64>,
}

fn decode_notification_action(
    action: &str,
    context: NotificationActionContext,
) -> Option<NotificationAction> {
    match action {
        ACTION_EDIT_RULE | ACTION_DEFAULT => Some(NotificationAction::EditRule {
            rule_id: context.rule_id,
        }),
        ACTION_RELOAD_CONFIG => Some(NotificationAction::ReloadConfig),
        ACTION_UNDO => context
            .transform_id
            .map(|transform_id| NotificationAction::Undo { transform_id }),
        ACTION_DISABLE_RULE => context
            .rule_id
            .zip(context.disable_seconds)
            .map(|(rule_id, seconds)| NotificationAction::DisableRule { rule_id, seconds }),
        _ => None,
    }
}

fn handle_notification_response(response: &UNNotificationResponse, commands: &ActionSink) {
    let action = response.actionIdentifier().to_string();
    let notification_id = response.notification().request().identifier().to_string();
    log::info!("notification action received notification_id={notification_id} action={action}");

    let context = NotificationActionContext {
        transform_id: user_info_value(response, USER_INFO_TRANSFORM_ID)
            .and_then(|value| value.parse().ok()),
        rule_id: user_info_value(response, USER_INFO_RULE_ID),
        disable_seconds: user_info_value(response, USER_INFO_DISABLE_SECONDS)
            .and_then(|value| value.parse().ok()),
    };
    let command = decode_notification_action(&action, context);

    let Some(command) = command else {
        log::info!(
            "notification action missing context notification_id={notification_id} action={action}"
        );
        return;
    };
    commands(command);
}

fn user_info_value(response: &UNNotificationResponse, key: &str) -> Option<String> {
    let user_info = response.notification().request().content().userInfo();
    let user_info = unsafe { user_info.cast_unchecked::<NSString, NSString>() };
    user_info
        .objectForKey(&NSString::from_str(key))
        .map(|value| value.to_string())
}

fn status_user_info(
    notification: &StartupNotification,
) -> Option<Retained<NSDictionary<NSString, NSString>>> {
    if notification.edit_target.is_none() && notification.reload_request_path.is_none() {
        return None;
    }

    let mut keys = Vec::new();
    let mut values = Vec::new();
    if let Some(edit_target) = &notification.edit_target {
        keys.push(NSString::from_str(USER_INFO_EDIT_PATH));
        values.push(NSString::from_str(&edit_target.path));
        if let Some(line) = edit_target.line {
            keys.push(NSString::from_str(USER_INFO_EDIT_LINE));
            values.push(NSString::from_str(&line.to_string()));
        }
    }
    if let Some(path) = &notification.reload_request_path {
        keys.push(NSString::from_str(USER_INFO_RELOAD_REQUEST_PATH));
        values.push(NSString::from_str(&path.display().to_string()));
    }

    let key_refs = keys.iter().map(|key| &**key).collect::<Vec<_>>();
    Some(NSDictionary::from_retained_objects(&key_refs, &values))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_undo_with_transform_id() {
        let transform_id = Uuid::new_v4();
        let command = decode_notification_action(
            ACTION_UNDO,
            NotificationActionContext {
                transform_id: Some(transform_id),
                ..NotificationActionContext::default()
            },
        );

        assert_eq!(command, Some(NotificationAction::Undo { transform_id }));
    }

    #[test]
    fn rejects_actions_with_missing_context() {
        assert_eq!(
            decode_notification_action(ACTION_UNDO, NotificationActionContext::default()),
            None
        );
        assert_eq!(
            decode_notification_action(
                ACTION_DISABLE_RULE,
                NotificationActionContext {
                    rule_id: Some("rule".into()),
                    ..NotificationActionContext::default()
                },
            ),
            None
        );
    }

    #[test]
    fn decodes_disable_and_default_click() {
        assert_eq!(
            decode_notification_action(
                ACTION_DISABLE_RULE,
                NotificationActionContext {
                    rule_id: Some("rule".into()),
                    disable_seconds: Some(60),
                    ..NotificationActionContext::default()
                },
            ),
            Some(NotificationAction::DisableRule {
                rule_id: "rule".into(),
                seconds: 60,
            })
        );
        assert_eq!(
            decode_notification_action(
                ACTION_DEFAULT,
                NotificationActionContext {
                    rule_id: Some("rule".into()),
                    ..NotificationActionContext::default()
                },
            ),
            Some(NotificationAction::EditRule {
                rule_id: Some("rule".into()),
            })
        );
    }

    #[test]
    fn category_set_is_complete_before_delivery() {
        assert_eq!(
            registered_category_ids(Some(600)),
            vec![
                CATEGORY_ID_WITHOUT_DISABLE,
                CATEGORY_ID_STATUS_WITH_RELOAD,
                CATEGORY_ID_WITH_DISABLE,
            ]
        );
        assert_eq!(
            registered_category_ids(None),
            vec![CATEGORY_ID_WITHOUT_DISABLE, CATEGORY_ID_STATUS_WITH_RELOAD]
        );
    }

    #[test]
    fn transform_category_matches_available_action() {
        assert_eq!(transform_category_id(Some(600)), CATEGORY_ID_WITH_DISABLE);
        assert_eq!(transform_category_id(None), CATEGORY_ID_WITHOUT_DISABLE);
    }
}
