use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use windows::core::{Interface, GUID, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, PROPERTYKEY};
use windows::Win32::System::Com::StructuredStorage::{InitPropVariantFromCLSID, PROPVARIANT};
use windows::Win32::System::Com::{
    CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ, STGM_READWRITE,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyW, RegDeleteTreeW, RegOpenKeyExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ,
};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    IShellLinkW, SetCurrentProcessExplicitAppUserModelID, ShellLink, SLGP_RAWPATH,
};

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

    // Scoop-generated shortcuts only carry the basic target, so the app
    // enriches an existing per-user Start Menu entry when one points at this
    // executable. Best-effort: a broken Start Menu (redirected profile,
    // endpoint policy) must not prevent the app from starting.
    if !machine_registered {
        if let Err(error) = ensure_start_menu_shortcut(executable) {
            crate::logging::event(format!(
                "start menu shortcut registration failed: {error:#}"
            ));
        }
    }
    Ok(())
}

pub fn set_process_app_user_model_id() -> Result<()> {
    let app_user_model_id = wide_null(APP_USER_MODEL_ID);
    unsafe {
        SetCurrentProcessExplicitAppUserModelID(PCWSTR(app_user_model_id.as_ptr()))
            .context("set Windows process AppUserModelID")?;
    }
    Ok(())
}

fn machine_registration_exists() -> bool {
    let subkey = wide_null(format!(
        "Software\\Classes\\CLSID\\{TOAST_ACTIVATOR_CLSID_STRING}\\LocalServer32"
    ));
    let mut key = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            Some(0),
            KEY_READ,
            &mut key,
        )
    };
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
    let subkey = wide_null(subkey);
    let status = unsafe { RegDeleteTreeW(root, PCWSTR(subkey.as_ptr())) };
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
    let subkey = wide_null(subkey);
    let mut key = HKEY::default();
    let status = unsafe { RegCreateKeyW(root, PCWSTR(subkey.as_ptr()), &mut key) };
    if status != ERROR_SUCCESS {
        bail!("create Windows registration key: Win32 error {}", status.0);
    }

    let value_name = value_name.map(wide_null);
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

fn ensure_start_menu_shortcut(executable: &Path) -> Result<()> {
    if let Some(shortcut_path) = find_shortcut_for_executable(executable)? {
        crate::logging::event(format!(
            "updating Windows Start Menu shortcut {}",
            shortcut_path.display()
        ));
        update_start_menu_shortcut(&shortcut_path)?;
        remove_redundant_app_shortcut(&shortcut_path)?;
    } else {
        create_start_menu_shortcut(executable)?;
    }
    Ok(())
}

fn find_shortcut_for_executable(executable: &Path) -> Result<Option<PathBuf>> {
    let programs_dir = start_menu_programs_dir()?;
    if !programs_dir.exists() {
        return Ok(None);
    }

    let app_shortcut_path = start_menu_shortcut_path()?;
    let mut matching_shortcuts = Vec::new();
    for shortcut_path in shortcut_paths_under(&programs_dir)? {
        match shortcut_target(&shortcut_path) {
            Ok(target) if same_path(&target, executable) => matching_shortcuts.push(shortcut_path),
            Ok(_) => {}
            Err(error) => crate::logging::event(format!(
                "failed to inspect Start Menu shortcut {}: {error:#}",
                shortcut_path.display()
            )),
        }
    }
    Ok(preferred_shortcut(matching_shortcuts, &app_shortcut_path))
}

fn preferred_shortcut(mut shortcuts: Vec<PathBuf>, app_shortcut_path: &Path) -> Option<PathBuf> {
    shortcuts.sort_by_key(|path| path_text_eq(path, app_shortcut_path));
    shortcuts.into_iter().next()
}

fn remove_redundant_app_shortcut(preferred_shortcut: &Path) -> Result<()> {
    let app_shortcut_path = start_menu_shortcut_path()?;
    if path_text_eq(preferred_shortcut, &app_shortcut_path) || !app_shortcut_path.exists() {
        return Ok(());
    }

    crate::logging::event(format!(
        "removing redundant Windows Start Menu shortcut {}",
        app_shortcut_path.display()
    ));
    fs::remove_file(&app_shortcut_path).with_context(|| {
        format!(
            "remove redundant Windows Start Menu shortcut {}",
            app_shortcut_path.display()
        )
    })?;
    if let Some(parent) = app_shortcut_path.parent() {
        match fs::remove_dir(parent) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "remove empty Windows Start Menu directory {}",
                        parent.display()
                    )
                });
            }
        }
    }
    Ok(())
}

fn update_start_menu_shortcut(shortcut_path: &Path) -> Result<()> {
    let shell_link = load_shell_link(shortcut_path, STGM_READWRITE).with_context(|| {
        format!(
            "load Windows Start Menu shortcut {}",
            shortcut_path.display()
        )
    })?;
    set_shortcut_properties(&shell_link)?;

    let persist_file: IPersistFile = shell_link
        .cast()
        .context("open Windows shortcut persistence interface")?;
    let shortcut_path = path_wide_null(shortcut_path);
    unsafe {
        persist_file
            .Save(PCWSTR(shortcut_path.as_ptr()), true)
            .context("save Windows Start Menu shortcut")?;
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
    let executable = path_wide_null(executable);
    let description = wide_null(DISPLAY_NAME);
    unsafe {
        shell_link
            .SetPath(PCWSTR(executable.as_ptr()))
            .context("set Windows Start Menu shortcut target")?;
        shell_link
            .SetDescription(PCWSTR(description.as_ptr()))
            .context("set Windows Start Menu shortcut description")?;
    }

    set_shortcut_properties(&shell_link)?;

    let persist_file: IPersistFile = shell_link
        .cast()
        .context("open Windows shortcut persistence interface")?;
    let shortcut_path = path_wide_null(&shortcut_path);
    unsafe {
        persist_file
            .Save(PCWSTR(shortcut_path.as_ptr()), true)
            .context("save Windows Start Menu shortcut")?;
    }
    Ok(())
}

fn set_shortcut_properties(shell_link: &IShellLinkW) -> Result<()> {
    let property_store: IPropertyStore = shell_link
        .cast()
        .context("open Windows shortcut property store")?;
    let app_id = PROPVARIANT::from(APP_USER_MODEL_ID);
    let activator = unsafe { InitPropVariantFromCLSID(&TOAST_ACTIVATOR_CLSID) }
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
    }
    Ok(())
}

fn load_shell_link(
    shortcut_path: &Path,
    mode: windows::Win32::System::Com::STGM,
) -> Result<IShellLinkW> {
    let shell_link: IShellLinkW =
        unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
            .context("create Windows shell link")?;
    let persist_file: IPersistFile = shell_link
        .cast()
        .context("open Windows shortcut persistence interface")?;
    let shortcut_path = path_wide_null(shortcut_path);
    unsafe {
        persist_file
            .Load(PCWSTR(shortcut_path.as_ptr()), mode)
            .context("load Windows shortcut")?;
    }
    Ok(shell_link)
}

fn shortcut_target(shortcut_path: &Path) -> Result<PathBuf> {
    let shell_link = load_shell_link(shortcut_path, STGM_READ)?;
    let mut target = vec![0u16; 32_768];
    unsafe {
        shell_link
            .GetPath(&mut target, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)
            .context("read Windows shortcut target")?;
    }
    let end = target
        .iter()
        .position(|ch| *ch == 0)
        .unwrap_or(target.len());
    Ok(PathBuf::from(String::from_utf16_lossy(&target[..end])))
}

fn shortcut_paths_under(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    collect_shortcut_paths(root, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn collect_shortcut_paths(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)
        .with_context(|| format!("read Start Menu directory {}", directory.display()))?
    {
        let entry =
            entry.with_context(|| format!("read Start Menu entry in {}", directory.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read Start Menu entry type {}", path.display()))?;
        if file_type.is_dir() {
            collect_shortcut_paths(&path, paths)?;
        } else if file_type.is_file() && is_shortcut_path(&path) {
            paths.push(path);
        }
    }
    Ok(())
}

fn is_shortcut_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) if path_text_eq(&left, &right) => return true,
        _ => {}
    }
    path_text_eq(left, right)
}

fn path_text_eq(left: &Path, right: &Path) -> bool {
    normalized_path_text(left).eq_ignore_ascii_case(&normalized_path_text(right))
}

fn normalized_path_text(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
}

fn wide_null(value: impl AsRef<str>) -> Vec<u16> {
    value
        .as_ref()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

fn path_wide_null(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn start_menu_shortcut_path() -> Result<PathBuf> {
    Ok(start_menu_programs_dir()?
        .join(DISPLAY_NAME)
        .join(format!("{DISPLAY_NAME}.lnk")))
}

fn start_menu_programs_dir() -> Result<PathBuf> {
    let app_data = std::env::var_os("APPDATA").context("APPDATA is unavailable")?;
    Ok(start_menu_programs_dir_from(Path::new(&app_data)))
}

#[cfg(test)]
fn start_menu_shortcut_path_from(app_data: &Path) -> PathBuf {
    start_menu_programs_dir_from(app_data)
        .join(DISPLAY_NAME)
        .join(format!("{DISPLAY_NAME}.lnk"))
}

fn start_menu_programs_dir_from(app_data: &Path) -> PathBuf {
    app_data
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
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

    #[test]
    fn shortcut_discovery_is_recursive_and_case_insensitive() {
        let directory = tempfile::tempdir().unwrap();
        let programs = directory.path().join("Programs");
        let scoop = programs.join("Scoop Apps").join("Clipboard Transformer");
        fs::create_dir_all(&scoop).unwrap();
        fs::write(scoop.join("Clipboard Transformer.LNK"), []).unwrap();
        fs::write(programs.join("not-a-shortcut.txt"), []).unwrap();

        let shortcuts = shortcut_paths_under(&programs).unwrap();

        assert_eq!(shortcuts, vec![scoop.join("Clipboard Transformer.LNK")]);
    }

    #[test]
    fn path_comparison_matches_windows_text_variants() {
        assert!(path_text_eq(
            Path::new(
                r"\\?\C:\Users\test\scoop\apps\clipboard-transformer\current\Clipboard Transformer.exe"
            ),
            Path::new(
                r"c:/users/test/scoop/apps/clipboard-transformer/current/clipboard transformer.exe"
            ),
        ));
    }

    #[test]
    fn existing_non_app_shortcut_is_preferred_over_fallback() {
        let app_shortcut = PathBuf::from(
            r"C:\Users\test\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Clipboard Transformer\Clipboard Transformer.lnk",
        );
        let scoop_shortcut = PathBuf::from(
            r"C:\Users\test\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Scoop Apps\Clipboard Transformer\Clipboard Transformer.lnk",
        );

        let preferred = preferred_shortcut(
            vec![app_shortcut.clone(), scoop_shortcut.clone()],
            &app_shortcut,
        );

        assert_eq!(preferred, Some(scoop_shortcut));
    }
}
