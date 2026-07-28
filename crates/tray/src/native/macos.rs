use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr;

use anyhow::{Context, Result};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{
    define_class, msg_send, sel, ClassType, DeclaredClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSBitmapFormat, NSBitmapImageRep, NSControlStateValueOff, NSControlStateValueOn,
    NSEventModifierFlags, NSImage, NSMenu, NSMenuDelegate, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSSize, NSString};

use crate::{
    accelerator_model, AcceleratorKey, ActionSink, TrayAction, TrayMenuItem, TrayMenuSource,
    TrayPlatform,
};

struct MenuControllerIvars {
    commands: ActionSink,
    menu: TrayMenuSource,
    command_by_tag: RefCell<HashMap<isize, TrayAction>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "ClipboardTransformerMenuController"]
    #[thread_kind = MainThreadOnly]
    #[ivars = MenuControllerIvars]
    struct MenuController;

    unsafe impl NSObjectProtocol for MenuController {}

    unsafe impl NSMenuDelegate for MenuController {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            self.rebuild(menu);
        }
    }

    impl MenuController {
        #[unsafe(method(performTrayCommand:))]
        fn perform_tray_command(&self, sender: &NSMenuItem) {
            let command = self.ivars().command_by_tag.borrow().get(&sender.tag()).cloned();
            if let Some(command) = command {
                log::info!("tray menu command selected");
                (self.ivars().commands)(command);
            }
        }
    }
);

impl MenuController {
    fn new(mtm: MainThreadMarker, commands: ActionSink, menu: TrayMenuSource) -> Retained<Self> {
        let this = mtm.alloc().set_ivars(MenuControllerIvars {
            commands,
            menu,
            command_by_tag: RefCell::new(HashMap::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn rebuild(&self, menu: &NSMenu) {
        // Called from `menuNeedsUpdate:`, so relative timestamps are current.
        let model = (self.ivars().menu)();
        self.ivars().command_by_tag.borrow_mut().clear();
        menu.removeAllItems();
        let mut next_tag = 1isize;
        append_items(menu, &model.items, self, &mut next_tag);
    }
}

pub struct MacosTray {
    status_item: Retained<NSStatusItem>,
    _menu: Retained<NSMenu>,
    _controller: Retained<MenuController>,
}

impl MacosTray {
    pub fn new(commands: ActionSink, menu_source: TrayMenuSource) -> Result<Self> {
        let mtm = MainThreadMarker::new().context("native macOS tray requires the main thread")?;
        // The controller owns the source outright: nothing replaces it, so there
        // is no reason for the tray to keep a second handle.
        let controller = MenuController::new(mtm, commands, menu_source);
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        menu.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
        controller.rebuild(&menu);

        let status_item =
            NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        status_item.setMenu(Some(&menu));
        let button = status_item
            .button(mtm)
            .context("native macOS status item has no button")?;
        button.setToolTip(Some(&NSString::from_str("Clipboard Transformer")));
        let icon = status_icon(mtm)?;
        button.setImage(Some(&icon));

        log::info!("native macOS tray initialized");
        Ok(Self {
            status_item,
            _menu: menu,
            _controller: controller,
        })
    }
}

impl Drop for MacosTray {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.status_item);
    }
}

fn append_items(
    menu: &NSMenu,
    items: &[TrayMenuItem],
    controller: &MenuController,
    next_tag: &mut isize,
) {
    let mtm = MainThreadMarker::from(menu);
    for item in items {
        let TrayMenuItem::Entry(item) = item else {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            continue;
        };
        if !item.visible {
            continue;
        }

        let supports_subtitle: bool = unsafe {
            msg_send![
                NSMenuItem::class(),
                instancesRespondToSelector: sel!(setSubtitle:)
            ]
        };
        let has_subtitle = item.label.has_subtitle() && supports_subtitle;
        let title = if has_subtitle {
            item.label.title_for(TrayPlatform::Macos)
        } else {
            item.label.single_line_for(TrayPlatform::Macos)
        };
        let key = item.accelerator.map(accelerator_key).unwrap_or_default();
        let native = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str(title),
                None,
                &NSString::from_str(key),
            )
        };
        native.setEnabled(item.enabled);
        if has_subtitle {
            native.setSubtitle(
                item.label
                    .subtitle_for(TrayPlatform::Macos)
                    .map(NSString::from_str)
                    .as_deref(),
            );
        }
        if let Some(checked) = item.checked {
            native.setState(if checked {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
        if let Some(accelerator) = item.accelerator {
            native.setKeyEquivalentModifierMask(accelerator_modifiers(accelerator));
        }
        if let Some(symbol) = item
            .icon
            .and_then(|icon| icon.name_for(TrayPlatform::Macos))
        {
            let description = NSString::from_str(title);
            if let Some(icon) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(symbol),
                Some(&description),
            ) {
                native.setImage(Some(&icon));
            }
        }

        if item.children.is_empty() {
            if let Some(command) = &item.command {
                let tag = *next_tag;
                *next_tag += 1;
                controller
                    .ivars()
                    .command_by_tag
                    .borrow_mut()
                    .insert(tag, command.clone());
                native.setTag(tag);
                unsafe {
                    native.setTarget(Some(controller));
                    native.setAction(Some(sel!(performTrayCommand:)));
                }
            }
        } else {
            let submenu = NSMenu::new(mtm);
            submenu.setAutoenablesItems(false);
            append_items(&submenu, &item.children, controller, next_tag);
            native.setSubmenu(Some(&submenu));
        }
        menu.addItem(&native);
    }
}

fn accelerator_key(accelerator: crate::MenuAccelerator) -> &'static str {
    match accelerator_model(accelerator).key {
        AcceleratorKey::C => "c",
        AcceleratorKey::O => "o",
        AcceleratorKey::Q => "q",
        AcceleratorKey::R => "r",
        AcceleratorKey::F4 => "",
    }
}

fn accelerator_modifiers(accelerator: crate::MenuAccelerator) -> NSEventModifierFlags {
    let model = accelerator_model(accelerator);
    let mut modifiers = NSEventModifierFlags::empty();
    if model.command {
        modifiers |= NSEventModifierFlags::Command;
    }
    if model.control {
        modifiers |= NSEventModifierFlags::Control;
    }
    if model.alt {
        modifiers |= NSEventModifierFlags::Option;
    }
    if model.shift {
        modifiers |= NSEventModifierFlags::Shift;
    }
    modifiers
}

fn status_icon(mtm: MainThreadMarker) -> Result<Retained<NSImage>> {
    let pixels = crate::macos_template_icon();
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            mtm.alloc(),
            ptr::null_mut(),
            pixels.width as isize,
            pixels.height as isize,
            8,
            4,
            true,
            false,
            objc2_app_kit::NSDeviceRGBColorSpace,
            NSBitmapFormat::AlphaNonpremultiplied,
            (pixels.width * 4) as isize,
            32,
        )
    }
    .context("create macOS tray bitmap")?;
    unsafe {
        ptr::copy_nonoverlapping(pixels.rgba.as_ptr(), bitmap.bitmapData(), pixels.rgba.len());
    }
    let image = NSImage::initWithSize(
        mtm.alloc(),
        NSSize::new(pixels.width as f64, pixels.height as f64),
    );
    image.addRepresentation(&bitmap);
    image.setTemplate(true);
    Ok(image)
}

impl MacosTray {
    /// Re-checks chrome that no click reports. macOS uses a template image, which the system recolors itself, so
    /// there is nothing to poll here.
    pub fn poll_chrome(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}
