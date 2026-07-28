#![allow(non_snake_case)]

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use windows::core::{implement, interface};
use windows::Win32::Foundation::{CLASS_E_NOAGGREGATION, E_POINTER};
use windows::Win32::System::Com::{
    CoInitializeEx, CoRegisterClassObject, CoRevokeClassObject, CoUninitialize, IClassFactory,
    IClassFactory_Impl, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, REGCLS_MULTIPLEUSE,
};
use windows_core::{
    Error as WindowsError, IUnknown, IUnknown_Vtbl, Interface, Ref, BOOL, GUID, HRESULT, PCWSTR,
};

use crate::app::AppCommand;
use crate::logging;

pub const APP_USER_MODEL_ID: &str = "dev.jag-k.clipboard-transformer";
pub const TOAST_ACTIVATOR_CLSID: GUID = GUID::from_u128(0xb87b8c6d_2489_4a7d_9efa_d02c54dd2390);
pub const TOAST_ACTIVATOR_CLSID_STRING: &str = "{B87B8C6D-2489-4A7D-9EFA-D02C54DD2390}";
pub const TOAST_ACTIVATED_ARGUMENT: &str = "-ToastActivated";

#[repr(C)]
struct NotificationUserInputData {
    key: PCWSTR,
    value: PCWSTR,
}

#[interface("53E31837-6600-4A81-9395-75CFFE746F94")]
unsafe trait INotificationActivationCallback: IUnknown {
    unsafe fn Activate(
        &self,
        app_user_model_id: PCWSTR,
        invoked_args: PCWSTR,
        data: *const NotificationUserInputData,
        count: u32,
    ) -> HRESULT;
}

#[derive(Default)]
struct ActivationRouter {
    sender: Option<Sender<AppCommand>>,
    pending: VecDeque<AppCommand>,
}

fn router() -> &'static Mutex<ActivationRouter> {
    static ROUTER: OnceLock<Mutex<ActivationRouter>> = OnceLock::new();
    ROUTER.get_or_init(|| Mutex::new(ActivationRouter::default()))
}

pub fn attach_command_sender(sender: Sender<AppCommand>) {
    let Ok(mut router) = router().lock() else {
        logging::event("Windows toast activation router is poisoned");
        return;
    };
    while let Some(command) = router.pending.pop_front() {
        if sender.send(command).is_err() {
            logging::event("Windows notification command channel unavailable");
            return;
        }
    }
    router.sender = Some(sender);
}

fn dispatch_argument(argument: &str) {
    let Some(command) = command_from_argument(argument) else {
        logging::event("ignored unsupported Windows toast activation argument");
        return;
    };
    let Ok(mut router) = router().lock() else {
        logging::event("Windows toast activation router is poisoned");
        return;
    };
    if let Some(sender) = router.sender.as_ref() {
        if sender.send(command).is_err() {
            router.sender = None;
        }
    } else {
        router.pending.push_back(command);
    }
}

#[implement(INotificationActivationCallback)]
struct ToastActivator;

impl INotificationActivationCallback_Impl for ToastActivator_Impl {
    unsafe fn Activate(
        &self,
        app_user_model_id: PCWSTR,
        invoked_args: PCWSTR,
        _data: *const NotificationUserInputData,
        _count: u32,
    ) -> HRESULT {
        let app_user_model_id = unsafe { app_user_model_id.to_string() }.unwrap_or_default();
        if app_user_model_id != APP_USER_MODEL_ID {
            logging::event("ignored Windows toast activation for another AppUserModelID");
            return HRESULT(0);
        }
        let argument = unsafe { invoked_args.to_string() }.unwrap_or_default();
        dispatch_argument(&argument);
        HRESULT(0)
    }
}

#[implement(IClassFactory)]
struct ToastActivatorFactory;

impl IClassFactory_Impl for ToastActivatorFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<'_, IUnknown>,
        interface_id: *const GUID,
        object: *mut *mut c_void,
    ) -> windows::core::Result<()> {
        if outer.is_some() {
            return Err(WindowsError::from_hresult(CLASS_E_NOAGGREGATION));
        }
        if interface_id.is_null() || object.is_null() {
            return Err(WindowsError::from_hresult(E_POINTER));
        }
        let callback: INotificationActivationCallback = ToastActivator.into();
        let unknown: IUnknown = callback.cast()?;
        unsafe {
            (Interface::vtable(&unknown).QueryInterface)(
                Interface::as_raw(&unknown),
                interface_id,
                object,
            )
            .ok()
        }
    }

    fn LockServer(&self, _lock: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}

pub struct ClassObjectRegistration {
    cookie: u32,
    com_initialized: bool,
    _factory: IClassFactory,
}

impl ClassObjectRegistration {
    pub fn register() -> Result<Self> {
        let initialize_result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let com_initialized = initialize_result.is_ok();
        if initialize_result.is_err()
            && initialize_result != windows::Win32::Foundation::RPC_E_CHANGED_MODE
        {
            initialize_result
                .ok()
                .context("initialize COM for Windows toast activation")?;
        }

        let factory: IClassFactory = ToastActivatorFactory.into();
        let cookie = unsafe {
            CoRegisterClassObject(
                &TOAST_ACTIVATOR_CLSID,
                &factory,
                CLSCTX_LOCAL_SERVER,
                REGCLS_MULTIPLEUSE,
            )
            .context("register Windows toast COM class object")?
        };
        Ok(Self {
            cookie,
            com_initialized,
            _factory: factory,
        })
    }
}

impl Drop for ClassObjectRegistration {
    fn drop(&mut self) {
        unsafe {
            let _ = CoRevokeClassObject(self.cookie);
            if self.com_initialized {
                CoUninitialize();
            }
        }
    }
}

pub fn command_from_argument(argument: &str) -> Option<AppCommand> {
    if argument == "reload" {
        return Some(AppCommand::ReloadConfig);
    }
    if argument == "open-config" {
        return Some(AppCommand::OpenConfig);
    }
    if let Some(transform_id) = argument.strip_prefix("undo:") {
        return transform_id
            .parse()
            .ok()
            .map(|transform_id| AppCommand::Undo { transform_id });
    }
    if let Some(rule_id) = argument.strip_prefix("edit:") {
        return Some(AppCommand::EditRule {
            rule_id: (!rule_id.is_empty()).then(|| rule_id.to_string()),
        });
    }
    if let Some(payload) = argument.strip_prefix("disable:") {
        let (seconds, rule_id) = payload.split_once(':')?;
        if rule_id.is_empty() {
            return None;
        }
        return Some(AppCommand::DisableRule {
            rule_id: rule_id.to_string(),
            seconds: seconds.parse().ok()?,
        });
    }
    None
}

pub fn toast_tag(notification_id: &str) -> String {
    let hash = notification_id
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_supported_activation_commands() {
        let transform_id = uuid::Uuid::new_v4();
        assert_eq!(
            command_from_argument(&format!("undo:{transform_id}")),
            Some(AppCommand::Undo { transform_id })
        );
        assert_eq!(
            command_from_argument("disable:60:rule:with:colons"),
            Some(AppCommand::DisableRule {
                rule_id: "rule:with:colons".into(),
                seconds: 60,
            })
        );
        assert_eq!(command_from_argument("quit"), None);
        assert_eq!(command_from_argument("disable:60:"), None);
    }

    #[test]
    fn toast_tags_are_short_stable_and_distinct() {
        let first = toast_tag("clipboard-transformer-11111111-1111-1111-1111-111111111111");
        assert_eq!(
            first,
            toast_tag("clipboard-transformer-11111111-1111-1111-1111-111111111111")
        );
        assert_eq!(first.len(), 16);
        assert_ne!(
            first,
            toast_tag("clipboard-transformer-22222222-2222-2222-2222-222222222222")
        );
    }
}
