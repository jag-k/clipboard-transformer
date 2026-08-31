use anyhow::{Context, Result};
use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};

use crate::{
    accelerator_model, AcceleratorKey, ActionSink, TrayAction, TrayMenuItem, TrayMenuSource,
    TrayOptions, TrayPlatform,
};

struct LinuxTrayState {
    commands: ActionSink,
    menu: TrayMenuSource,
}

impl ksni::Tray for LinuxTrayState {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "dev.jag-k.clipboard-transformer".into()
    }

    fn title(&self) -> String {
        "Clipboard Transformer".into()
    }

    fn icon_name(&self) -> String {
        "clipboard-transformer-symbolic".into()
    }

    fn icon_theme_path(&self) -> String {
        std::env::var_os("APPDIR")
            .filter(|appdir| !appdir.is_empty())
            .map(std::path::PathBuf::from)
            .map(|appdir| appdir.join("usr/share/icons"))
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let pixels = crate::themed_icon(false);
        pixels
            .validate()
            .expect("generated Linux tray icon payload is valid");
        let mut argb = pixels
            .rgba8()
            .expect("Linux tray fallback icon uses RGBA8 pixels")
            .to_vec();
        for pixel in argb.as_chunks_mut::<4>().0 {
            pixel.rotate_right(1);
        }
        vec![ksni::Icon {
            width: pixels.width as i32,
            height: pixels.height as i32,
            data: argb,
        }]
    }

    fn menu_about_to_show(&mut self) {
        // The host publishes a current snapshot through Handle::update. The
        // menu is rebuilt by ksni immediately after this root pre-open hook.
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        // ksni calls this after the pre-open hook, so relative timestamps are
        // formatted when the menu opens rather than when the host last updated.
        let model = (self.menu)();
        render_items(&model.items)
    }
}

pub struct LinuxTray {
    handle: Handle<LinuxTrayState>,
}

impl LinuxTray {
    pub fn new(
        commands: ActionSink,
        menu_source: TrayMenuSource,
        options: TrayOptions,
    ) -> Result<Self> {
        let handle = LinuxTrayState {
            commands,
            menu: menu_source,
        }
        .disable_dbus_name(options.sandboxed)
        .spawn()
        .context("start native Linux StatusNotifierItem tray")?;
        log::info!("native Linux StatusNotifierItem tray initialized");
        Ok(Self { handle })
    }
}

impl Drop for LinuxTray {
    fn drop(&mut self) {
        self.handle.shutdown().wait();
    }
}

fn render_items(items: &[TrayMenuItem]) -> Vec<ksni::MenuItem<LinuxTrayState>> {
    items
        .iter()
        .filter_map(|item| match item {
            TrayMenuItem::Separator => Some(ksni::MenuItem::Separator),
            TrayMenuItem::Entry(item) if !item.visible => None,
            TrayMenuItem::Entry(item) if !item.children.is_empty() => Some(
                SubMenu {
                    label: dbus_label(item.label.single_line_for(TrayPlatform::Linux)),
                    enabled: item.enabled,
                    visible: item.visible,
                    icon_name: linux_icon_name(item),
                    shortcut: shortcut(item.accelerator),
                    submenu: render_items(&item.children),
                    ..Default::default()
                }
                .into(),
            ),
            TrayMenuItem::Entry(item) if item.checked.is_some() => {
                let command = item.command.clone();
                Some(
                    CheckmarkItem {
                        label: dbus_label(item.label.single_line_for(TrayPlatform::Linux)),
                        enabled: item.enabled,
                        visible: item.visible,
                        checked: item.checked.unwrap_or(false),
                        icon_name: linux_icon_name(item),
                        shortcut: shortcut(item.accelerator),
                        activate: Box::new(move |tray| send_command(tray, command.clone())),
                        ..Default::default()
                    }
                    .into(),
                )
            }
            TrayMenuItem::Entry(item) => {
                let command = item.command.clone();
                Some(
                    StandardItem {
                        label: dbus_label(item.label.single_line_for(TrayPlatform::Linux)),
                        enabled: item.enabled,
                        visible: item.visible,
                        icon_name: linux_icon_name(item),
                        shortcut: shortcut(item.accelerator),
                        activate: Box::new(move |tray| send_command(tray, command.clone())),
                        ..Default::default()
                    }
                    .into(),
                )
            }
        })
        .collect()
}

fn send_command(tray: &mut LinuxTrayState, command: Option<TrayAction>) {
    if let Some(command) = command {
        log::info!("tray menu command selected");
        (tray.commands)(command);
    }
}

fn linux_icon_name(item: &crate::TrayMenuEntry) -> String {
    item.icon
        .and_then(|icon| icon.name_for(TrayPlatform::Linux))
        .unwrap_or_default()
        .to_string()
}

fn shortcut(accelerator: Option<crate::MenuAccelerator>) -> Vec<Vec<String>> {
    let Some(accelerator) = accelerator else {
        return Vec::new();
    };
    let model = accelerator_model(accelerator);
    let mut keys = Vec::new();
    if model.command {
        keys.push("Super".into());
    }
    if model.control {
        keys.push("Control".into());
    }
    if model.alt {
        keys.push("Alt".into());
    }
    if model.shift {
        keys.push("Shift".into());
    }
    keys.push(
        match model.key {
            AcceleratorKey::C => "C",
            AcceleratorKey::O => "O",
            AcceleratorKey::Q => "Q",
            AcceleratorKey::R => "R",
            AcceleratorKey::F4 => "F4",
        }
        .into(),
    );
    vec![keys]
}

fn dbus_label(value: &str) -> String {
    value.replace('_', "__")
}

impl LinuxTray {
    /// Re-checks chrome that no click reports. The status icon is a themed name resolved by the host, so
    /// there is nothing to poll here.
    pub fn poll_chrome(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}
