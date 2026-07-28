use std::fs;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use windows::core::{Interface, GUID, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, PROPERTYKEY};
use windows::Win32::System::Com::StructuredStorage::{
    InitPropVariantFromCLSID, PropVariantClear, PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0,
    PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyW, RegDeleteTreeW, RegOpenKeyExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{IShellLinkW, SetCurrentProcessExplicitAppUserModelID, ShellLink};

use super::activation::{
    APP_USER_MODEL_ID, TOAST_ACTIVATED_ARGUMENT, TOAST_ACTIVATOR_CLSID,
    TOAST_ACTIVATOR_CLSID_STRING,
};

const DISPLAY_NAME: &str = "Clipboard Transformer";
const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};
const PKEY_APP_USER_MODEL_TOAST_ACTIVATOR_CLSID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 26,
};

pub fn ensure_desktop_registration(executable: &Path) -> Result<()> {
    set_process_app_user_model_id()?;
    let machine_registered = machine_registration_exists();
    if machine_registered {
        remove_current_user_registration()?;
    } else {
        register_current_user(executable)?;
    }

    // Installer-generated and Scoop-generated shortcuts only carry the basic
    // target and AppUserModelID, so the app maintains its own per-user
    // shortcut with the ToastActivatorCLSID property required for actionable
    // notifications. Best-effort: a broken Start Menu (redirected profile,
    // endpoint policy) must not prevent the app from starting.
    if !machine_registered {
        if let Err(error) = create_start_menu_shortcut(executable) {
            crate::logging::event(format!(
                "start menu shortcut registration failed: {error:#}"
            ));
        }
    }
    Ok(())
}

pub fn set_process_app_user_model_id() -> Result<()> {
    unsafe {
        SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_USER_MODEL_ID))
            .context("set Windows process AppUserModelID")?;
    }
    Ok(())
}

fn machine_registration_exists() -> bool {
    let subkey = HSTRING::from(format!(
        "Software\\Classes\\CLSID\\{TOAST_ACTIVATOR_CLSID_STRING}\\LocalServer32"
    ));
    let mut key = HKEY::default();
    let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, &subkey, Some(0), KEY_READ, &mut key) };
    if status == ERROR_SUCCESS {
        unsafe {
            let _ = RegCloseKey(key);
        }
        true
    } else {
        false
    }
}

fn register_current_user(executable: &Path) -> Result<()> {
    let command = format!("\"{}\" {TOAST_ACTIVATED_ARGUMENT}", executable.display());
    set_registry_string(
        HKEY_CURRENT_USER,
        &format!("Software\\Classes\\CLSID\\{TOAST_ACTIVATOR_CLSID_STRING}\\LocalServer32"),
        None,
        &command,
    )?;
    let app_id_key = format!("Software\\Classes\\AppUserModelId\\{APP_USER_MODEL_ID}");
    set_registry_string(
        HKEY_CURRENT_USER,
        &app_id_key,
        Some("DisplayName"),
        DISPLAY_NAME,
    )?;
    set_registry_string(
        HKEY_CURRENT_USER,
        &app_id_key,
        Some("CustomActivator"),
        TOAST_ACTIVATOR_CLSID_STRING,
    )
}

fn remove_current_user_registration() -> Result<()> {
    delete_registry_tree(
        HKEY_CURRENT_USER,
        &format!("Software\\Classes\\CLSID\\{TOAST_ACTIVATOR_CLSID_STRING}"),
    )?;
    delete_registry_tree(
        HKEY_CURRENT_USER,
        &format!("Software\\Classes\\AppUserModelId\\{APP_USER_MODEL_ID}"),
    )
}

fn delete_registry_tree(root: HKEY, subkey: &str) -> Result<()> {
    let status = unsafe { RegDeleteTreeW(root, &HSTRING::from(subkey)) };
    if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        bail!(
            "remove stale per-user Windows registration: Win32 error {}",
            status.0
        )
    }
}

fn set_registry_string(
    root: HKEY,
    subkey: &str,
    value_name: Option<&str>,
    value: &str,
) -> Result<()> {
    let subkey = HSTRING::from(subkey);
    let mut key = HKEY::default();
    let status = unsafe { RegCreateKeyW(root, &subkey, &mut key) };
    if status != ERROR_SUCCESS {
        bail!("create Windows registration key: Win32 error {}", status.0);
    }

    let value_name = value_name.map(HSTRING::from);
    let bytes = utf16_bytes(value);
    let status = unsafe {
        RegSetValueExW(
            key,
            value_name
                .as_ref()
                .map_or(PCWSTR::null(), |name| PCWSTR(name.as_ptr())),
            None,
            REG_SZ,
            Some(&bytes),
        )
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    if status != ERROR_SUCCESS {
        bail!("write Windows registration value: Win32 error {}", status.0);
    }
    Ok(())
}

fn create_start_menu_shortcut(executable: &Path) -> Result<()> {
    let shortcut_path = start_menu_shortcut_path()?;
    if let Some(parent) = shortcut_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create Windows Start Menu directory {}", parent.display()))?;
    }

    let shell_link: IShellLinkW =
        unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
            .context("create Windows Start Menu shell link")?;
    let executable = HSTRING::from(executable.as_os_str());
    unsafe {
        shell_link
            .SetPath(PCWSTR(executable.as_ptr()))
            .context("set Windows Start Menu shortcut target")?;
        shell_link
            .SetDescription(&HSTRING::from(DISPLAY_NAME))
            .context("set Windows Start Menu shortcut description")?;
    }

    let property_store: IPropertyStore = shell_link
        .cast()
        .context("open Windows shortcut property store")?;
    let app_id_text = HSTRING::from(APP_USER_MODEL_ID);
    let app_id = borrowed_string_property(&app_id_text);
    let mut activator = unsafe { InitPropVariantFromCLSID(&TOAST_ACTIVATOR_CLSID) }
        .context("create toast activator shortcut property")?;
    unsafe {
        property_store
            .SetValue(&PKEY_APP_USER_MODEL_ID, &app_id)
            .context("set shortcut AppUserModelID")?;
        property_store
            .SetValue(&PKEY_APP_USER_MODEL_TOAST_ACTIVATOR_CLSID, &activator)
            .context("set shortcut toast activator CLSID")?;
        property_store
            .Commit()
            .context("commit Windows shortcut properties")?;
        PropVariantClear(&mut activator).context("release toast activator shortcut property")?;
    }

    let persist_file: IPersistFile = shell_link
        .cast()
        .context("open Windows shortcut persistence interface")?;
    let shortcut_path = HSTRING::from(shortcut_path.as_os_str());
    unsafe {
        persist_file
            .Save(PCWSTR(shortcut_path.as_ptr()), true)
            .context("save Windows Start Menu shortcut")
    }
}

fn borrowed_string_property(value: &HSTRING) -> PROPVARIANT {
    PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_LPWSTR,
                Anonymous: PROPVARIANT_0_0_0 {
                    pwszVal: PWSTR(value.as_ptr().cast_mut()),
                },
                ..PROPVARIANT_0_0::default()
            }),
        },
    }
}

fn start_menu_shortcut_path() -> Result<PathBuf> {
    let app_data = std::env::var_os("APPDATA").context("APPDATA is unavailable")?;
    Ok(start_menu_shortcut_path_from(Path::new(&app_data)))
}

fn start_menu_shortcut_path_from(app_data: &Path) -> PathBuf {
    app_data
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join(DISPLAY_NAME)
        .join(format!("{DISPLAY_NAME}.lnk"))
}

fn utf16_bytes(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_shortcut_uses_per_user_start_menu() {
        let shortcut = start_menu_shortcut_path_from(Path::new(r"C:\Users\test\AppData\Roaming"));
        assert!(shortcut.ends_with(
            r"Microsoft\Windows\Start Menu\Programs\Clipboard Transformer\Clipboard Transformer.lnk"
        ));
    }
}
