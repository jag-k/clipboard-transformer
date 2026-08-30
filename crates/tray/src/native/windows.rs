use std::collections::HashMap;
use std::ptr;

use anyhow::{Context, Result};
use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::{FALSE, HWND, LPARAM, LRESULT, POINT, TRUE, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_GUID, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NIM_SETFOCUS, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, ChangeWindowMessageFilterEx, CreateIcon, CreatePopupMenu, CreateWindowExW,
    DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, GetCursorPos, GetMenuItemCount,
    LoadImageW, PostMessageW, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow,
    SetMenuItemInfoW, TrackPopupMenuEx, GWL_USERDATA, HICON, HMENU, IMAGE_BITMAP, LR_SHARED,
    MENUITEMINFOW, MF_CHECKED, MF_DISABLED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING,
    MIIM_BITMAP, MSGFLT_ALLOW, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD,
    WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_OVERLAPPED,
};

use crate::{
    accelerator_model, AcceleratorKey, ActionSink, TrayAction, TrayMenuItem, TrayMenuSource,
    TrayOptions, TrayPlatform,
};

const WM_TRAY_ICON: u32 = 0x8001;
const TRAY_ID: u32 = 1;
const NIN_KEYSELECT: u32 = 0x401;
const TRAY_GUID: GUID = GUID {
    data1: 0x9f96_2df1,
    data2: 0x2fd0,
    data3: 0x4db2,
    data4: [0xa4, 0x4f, 0x51, 0x44, 0x8e, 0xd7, 0xf2, 0xa1],
};

struct TrayData {
    commands: ActionSink,
    menu: TrayMenuSource,
    icon: HICON,
    dark_theme: bool,
    taskbar_created: u32,
}

pub struct WindowsTray {
    hwnd: HWND,
}

impl WindowsTray {
    pub fn new(
        commands: ActionSink,
        menu_source: TrayMenuSource,
        _options: TrayOptions,
    ) -> Result<Self> {
        unsafe {
            let instance = GetModuleHandleW(ptr::null());
            if instance.is_null() {
                anyhow::bail!(
                    "get native Windows tray module: {}",
                    std::io::Error::last_os_error()
                );
            }
            let class_name = wide("ClipboardTransformerTrayHost");
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class_name.as_ptr(),
                ..Default::default()
            };
            RegisterClassW(&class);

            let taskbar_name = wide("TaskbarCreated");
            let taskbar_created = RegisterWindowMessageW(taskbar_name.as_ptr());
            if taskbar_created == 0 {
                anyhow::bail!(
                    "register Windows TaskbarCreated message: {}",
                    std::io::Error::last_os_error()
                );
            }
            let dark_theme = prefers_dark_theme();
            let icon = create_icon(dark_theme)?;
            let hwnd = CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                class_name.as_ptr(),
                ptr::null(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null(),
            );
            if hwnd.is_null() {
                DestroyIcon(icon);
                anyhow::bail!(
                    "create native Windows tray owner: {}",
                    std::io::Error::last_os_error()
                );
            }
            let data = Box::new(TrayData {
                commands,
                menu: menu_source,
                icon,
                dark_theme,
                taskbar_created,
            });
            windows_sys::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
                hwnd,
                GWL_USERDATA,
                Box::into_raw(data) as isize,
            );
            ChangeWindowMessageFilterEx(hwnd, taskbar_created, MSGFLT_ALLOW, ptr::null_mut());
            if let Err(error) = register_icon(hwnd, icon) {
                DestroyWindow(hwnd);
                return Err(error);
            }
            log::info!("native Windows tray initialized");
            Ok(Self { hwnd })
        }
    }

    /// Re-checks the system light/dark preference and swaps the icon when it
    /// changed. Nothing here touches the menu, so an open menu is unaffected.
    pub fn poll_chrome(&mut self) -> Result<()> {
        unsafe {
            let data = tray_data_ptr(self.hwnd)
                .as_mut()
                .context("native Windows tray state unavailable")?;
            let dark_theme = prefers_dark_theme();
            if dark_theme != data.dark_theme {
                let icon = create_icon(dark_theme)?;
                let mut notification = notification_data(self.hwnd);
                notification.uFlags = NIF_GUID | NIF_ICON;
                notification.hIcon = icon;
                if Shell_NotifyIconW(NIM_MODIFY, &notification) == FALSE {
                    DestroyIcon(icon);
                    anyhow::bail!(
                        "update native Windows tray icon: {}",
                        std::io::Error::last_os_error()
                    );
                }
                DestroyIcon(data.icon);
                data.icon = icon;
                data.dark_theme = dark_theme;
            }
        }
        Ok(())
    }
}

impl Drop for WindowsTray {
    fn drop(&mut self) {
        unsafe {
            remove_icon(self.hwnd);
            DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let data = tray_data_ptr(hwnd);
    if data.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }

    if message == (*data).taskbar_created {
        let _ = register_icon(hwnd, (*data).icon);
        return 0;
    }
    match message {
        WM_TRAY_ICON => {
            let event = (lparam as u32) & 0xffff;
            if matches!(
                event,
                WM_CONTEXTMENU
                    | WM_RBUTTONUP
                    | WM_LBUTTONUP
                    | windows_sys::Win32::UI::Shell::NIN_SELECT
                    | NIN_KEYSELECT
            ) {
                show_menu(hwnd, data);
            }
            0
        }
        WM_CONTEXTMENU => {
            show_menu(hwnd, data);
            0
        }
        WM_DESTROY => {
            let data_ptr =
                windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWL_USERDATA)
                    as *mut TrayData;
            windows_sys::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(hwnd, GWL_USERDATA, 0);
            if !data_ptr.is_null() {
                let data = Box::from_raw(data_ptr);
                DestroyIcon(data.icon);
            }
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn show_menu(hwnd: HWND, data: *mut TrayData) {
    // TrackPopupMenuEx runs a nested message loop, so nothing borrowed from
    // TrayData may live across it. Neither the menu source nor the action sink
    // is clonable, and neither needs to be: the menu is fully built before the
    // call, and the sink is only reached after it returns.
    let Some((menu, commands)) = build_native_menu(&(*data).menu) else {
        return;
    };
    let mut cursor = POINT { x: 0, y: 0 };
    if GetCursorPos(&mut cursor) == FALSE {
        DestroyMenu(menu);
        return;
    }
    SetForegroundWindow(hwnd);
    let selected = TrackPopupMenuEx(
        menu,
        TPM_BOTTOMALIGN | TPM_LEFTALIGN | TPM_RETURNCMD | TPM_NONOTIFY,
        cursor.x,
        cursor.y,
        hwnd,
        ptr::null(),
    );
    PostMessageW(hwnd, WM_NULL, 0, 0);
    let focus = notification_data(hwnd);
    Shell_NotifyIconW(NIM_SETFOCUS, &focus);
    DestroyMenu(menu);

    if selected != 0 {
        if let Some(command) = commands.get(&(selected as u32)).cloned() {
            log::info!("tray menu command selected");
            ((*data).commands)(command);
        }
    }
}

unsafe fn build_native_menu(
    menu_source: &TrayMenuSource,
) -> Option<(HMENU, HashMap<u32, TrayAction>)> {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return None;
    }
    // Built when the popup is about to show, keeping relative timestamps current.
    let model = menu_source();
    let mut commands = HashMap::new();
    let mut next_id = 1000u32;
    append_items(menu, &model.items, &mut next_id, &mut commands);
    Some((menu, commands))
}

unsafe fn append_items(
    menu: HMENU,
    items: &[TrayMenuItem],
    next_id: &mut u32,
    commands: &mut HashMap<u32, TrayAction>,
) {
    for item in items {
        let TrayMenuItem::Entry(item) = item else {
            AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
            continue;
        };
        if !item.visible {
            continue;
        }
        let mut flags = MF_STRING;
        if !item.enabled {
            flags |= MF_DISABLED | MF_GRAYED;
        }
        if item.checked == Some(true) {
            flags |= MF_CHECKED;
        }
        let mut label = item
            .label
            .single_line_for(TrayPlatform::Windows)
            .to_string();
        if let Some(accelerator) = item.accelerator {
            label.push('\t');
            label.push_str(accelerator_text(accelerator));
        }
        let label = wide(&label);
        let position = GetMenuItemCount(menu);
        if item.children.is_empty() {
            let id = *next_id;
            *next_id += 1;
            if let Some(command) = &item.command {
                commands.insert(id, command.clone());
            }
            AppendMenuW(menu, flags, id as usize, label.as_ptr());
        } else {
            let submenu = CreatePopupMenu();
            if submenu.is_null() {
                continue;
            }
            append_items(submenu, &item.children, next_id, commands);
            AppendMenuW(menu, flags | MF_POPUP, submenu as usize, label.as_ptr());
        }
        if position >= 0 {
            set_menu_icon(menu, position as u32, item.icon);
        }
    }
}

unsafe fn set_menu_icon(menu: HMENU, position: u32, icon: Option<crate::TrayIcon>) {
    let Some(resource) = icon.and_then(|icon| icon.name_for(TrayPlatform::Windows)) else {
        return;
    };
    let resource = wide(resource);
    let bitmap = LoadImageW(
        GetModuleHandleW(ptr::null()),
        resource.as_ptr(),
        IMAGE_BITMAP,
        0,
        0,
        LR_SHARED,
    );
    if bitmap.is_null() {
        return;
    }
    let info = MENUITEMINFOW {
        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_BITMAP,
        hbmpItem: bitmap as _,
        ..Default::default()
    };
    SetMenuItemInfoW(menu, position, TRUE, &info);
}

fn accelerator_text(accelerator: crate::MenuAccelerator) -> &'static str {
    let model = accelerator_model(accelerator);
    match (model.control, model.alt, model.shift, model.key) {
        (true, false, false, AcceleratorKey::R) => "Ctrl+R",
        (true, false, false, AcceleratorKey::O) => "Ctrl+O",
        (true, false, true, AcceleratorKey::O) => "Ctrl+Shift+O",
        (true, false, false, AcceleratorKey::C) => "Ctrl+C",
        (false, true, false, AcceleratorKey::F4) => "Alt+F4",
        _ => "",
    }
}

unsafe fn register_icon(hwnd: HWND, icon: HICON) -> Result<()> {
    let mut data = notification_data(hwnd);
    data.uFlags = NIF_GUID | NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_TRAY_ICON;
    data.hIcon = icon;
    copy_wide(&mut data.szTip, "Clipboard Transformer");
    if Shell_NotifyIconW(NIM_ADD, &data) == FALSE {
        anyhow::bail!(
            "register native Windows tray icon: {}",
            std::io::Error::last_os_error()
        );
    }
    data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    if Shell_NotifyIconW(NIM_SETVERSION, &data) == FALSE {
        remove_icon(hwnd);
        anyhow::bail!(
            "set native Windows tray icon version: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

unsafe fn remove_icon(hwnd: HWND) {
    let data = notification_data(hwnd);
    Shell_NotifyIconW(NIM_DELETE, &data);
}

fn notification_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        guidItem: TRAY_GUID,
        ..Default::default()
    }
}

unsafe fn tray_data_ptr(hwnd: HWND) -> *mut TrayData {
    windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWL_USERDATA)
        as *mut TrayData
}

fn create_icon(dark_theme: bool) -> Result<HICON> {
    let pixels = crate::themed_icon(dark_theme);
    pixels.validate().map_err(|error| anyhow::anyhow!(error))?;
    let mut bgra = pixels
        .rgba8()
        .context("Windows tray icon must use RGBA8 pixels")?
        .to_vec();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        let alpha = u16::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    let mask_row_bytes = pixels.width.div_ceil(16) * 2;
    let mask = vec![0; (mask_row_bytes * pixels.height) as usize];
    let icon = unsafe {
        CreateIcon(
            ptr::null_mut(),
            pixels.width as i32,
            pixels.height as i32,
            1,
            32,
            mask.as_ptr(),
            bgra.as_ptr(),
        )
    };
    if icon.is_null() {
        anyhow::bail!(
            "create native Windows tray icon: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(icon)
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn copy_wide<const N: usize>(target: &mut [u16; N], value: &str) {
    for (destination, source) in target.iter_mut().zip(wide(value)) {
        *destination = source;
    }
}

/// Reads the Windows apps light/dark preference, which selects the tray icon.
pub(crate) fn prefers_dark_theme() -> bool {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

    let key = HSTRING::from("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let value = HSTRING::from("AppsUseLightTheme");
    let mut light_theme = 1u32;
    let mut size = std::mem::size_of_val(&light_theme) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            &value,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut light_theme as *mut u32).cast()),
            Some(&mut size),
        )
    };
    status == ERROR_SUCCESS && light_theme == 0
}
