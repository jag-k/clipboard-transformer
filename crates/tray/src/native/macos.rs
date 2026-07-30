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
    NSBitmapFormat, NSBitmapImageRep, NSCellImagePosition, NSControlStateValueOff,
    NSControlStateValueOn, NSEventModifierFlags, NSImage, NSMenu, NSMenuDelegate, NSMenuItem,
    NSSquareStatusItemLength, NSStatusBar, NSStatusItem,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSSize, NSString};

use crate::{
    accelerator_model, AcceleratorKey, ActionSink, TrayAction, TrayMenuEntry, TrayMenuItem,
    TrayMenuSource, TrayPlatform,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum NativeMenuKey {
    Entry(String),
    Separator(usize),
}

struct NativeMenuItem {
    key: NativeMenuKey,
    item: Retained<NSMenuItem>,
    submenu: Option<Retained<NSMenu>>,
    children: Vec<NativeMenuItem>,
}

struct MenuControllerIvars {
    commands: ActionSink,
    menu: TrayMenuSource,
    command_by_tag: RefCell<HashMap<isize, TrayAction>>,
    native_items: RefCell<Vec<NativeMenuItem>>,
    symbol_images: RefCell<HashMap<&'static str, Retained<NSImage>>>,
    supports_subtitle: bool,
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
            native_items: RefCell::new(Vec::new()),
            symbol_images: RefCell::new(HashMap::new()),
            supports_subtitle: unsafe {
                msg_send![
                    NSMenuItem::class(),
                    instancesRespondToSelector: sel!(setSubtitle:)
                ]
            },
        });
        unsafe { msg_send![super(this), init] }
    }

    fn rebuild(&self, menu: &NSMenu) {
        // Called from `menuNeedsUpdate:`, so relative timestamps are current.
        let model = (self.ivars().menu)();
        let mut command_by_tag = HashMap::new();
        let mut next_tag = 1isize;
        reconcile_items(
            menu,
            &mut self.ivars().native_items.borrow_mut(),
            &model.items,
            self,
            &mut next_tag,
            &mut command_by_tag,
        );

        // Swap both semantic dispatch tables only after the complete native
        // tree is consistent. AppKit can never observe a tag pointing at a
        // command from the previous model.
        *self.ivars().command_by_tag.borrow_mut() = command_by_tag;
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

        // This status item has no title, so give AppKit the icon-only square
        // contract explicitly. Variable length is intended for content-driven
        // items and can leave an image-only button without a visible slot.
        let status_item =
            NSStatusBar::systemStatusBar().statusItemWithLength(NSSquareStatusItemLength);
        // AppKit persists a status item's visibility and position by this
        // name. Keep it globally unique so the item cannot inherit another
        // application's saved menu-bar visibility.
        let autosave_name = NSString::from_str("dev.jag-k.clipboard-transformer.status-item");
        status_item.setAutosaveName(Some(&autosave_name));
        status_item.setMenu(Some(&menu));
        let button = status_item
            .button(mtm)
            .context("native macOS status item has no button")?;
        button.setToolTip(Some(&NSString::from_str("Clipboard Transformer")));
        let icon = status_icon(mtm)?;
        button.setImage(Some(&icon));
        button.setImagePosition(NSCellImagePosition::ImageOnly);

        let size = icon.size();
        log::info!(
            "native macOS tray initialized autosave_name={} visible={} length={} image={}x{}",
            autosave_name,
            status_item.isVisible(),
            status_item.length(),
            size.width,
            size.height
        );
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

fn reconcile_items(
    menu: &NSMenu,
    native_items: &mut Vec<NativeMenuItem>,
    model_items: &[TrayMenuItem],
    controller: &MenuController,
    next_tag: &mut isize,
    command_by_tag: &mut HashMap<isize, TrayAction>,
) {
    let mtm = MainThreadMarker::from(menu);
    for (index, model_item) in model_items.iter().enumerate() {
        let key = item_key(model_item, index);
        let existing_index = native_items.iter().position(|native| native.key == key);
        let mut native = match existing_index {
            Some(existing_index) => {
                let native = native_items.remove(existing_index);
                if existing_index != index {
                    menu.removeItem(&native.item);
                    menu.insertItem_atIndex(&native.item, index as isize);
                }
                native
            }
            None => {
                let native = create_native_item(mtm, key.clone(), model_item);
                menu.insertItem_atIndex(&native.item, index as isize);
                native
            }
        };

        match model_item {
            TrayMenuItem::Separator => {}
            TrayMenuItem::Entry(entry) => {
                update_native_entry(&mut native, entry, controller, next_tag, command_by_tag)
            }
        }
        native_items.insert(index, native);
    }

    while native_items.len() > model_items.len() {
        let stale = native_items.pop().expect("length was checked");
        menu.removeItem(&stale.item);
    }
}

fn item_key(item: &TrayMenuItem, index: usize) -> NativeMenuKey {
    match item {
        TrayMenuItem::Entry(entry) => NativeMenuKey::Entry(entry.id.clone()),
        TrayMenuItem::Separator => NativeMenuKey::Separator(index),
    }
}

fn create_native_item(
    mtm: MainThreadMarker,
    key: NativeMenuKey,
    model_item: &TrayMenuItem,
) -> NativeMenuItem {
    let item = match model_item {
        TrayMenuItem::Separator => NSMenuItem::separatorItem(mtm),
        TrayMenuItem::Entry(_) => unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::new(),
                None,
                &NSString::new(),
            )
        },
    };
    NativeMenuItem {
        key,
        item,
        submenu: None,
        children: Vec::new(),
    }
}

fn update_native_entry(
    native: &mut NativeMenuItem,
    item: &TrayMenuEntry,
    controller: &MenuController,
    next_tag: &mut isize,
    command_by_tag: &mut HashMap<isize, TrayAction>,
) {
    let has_subtitle = item.label.has_subtitle() && controller.ivars().supports_subtitle;
    let title = if has_subtitle {
        item.label.title_for(TrayPlatform::Macos)
    } else {
        item.label.single_line_for(TrayPlatform::Macos)
    };
    native.item.setTitle(&NSString::from_str(title));
    native.item.setEnabled(item.enabled);
    native.item.setHidden(!item.visible);
    native.item.setState(match item.checked {
        Some(true) => NSControlStateValueOn,
        Some(false) | None => NSControlStateValueOff,
    });
    if controller.ivars().supports_subtitle {
        native.item.setSubtitle(
            has_subtitle
                .then(|| item.label.subtitle_for(TrayPlatform::Macos))
                .flatten()
                .map(NSString::from_str)
                .as_deref(),
        );
    }

    let key = item.accelerator.map(accelerator_key).unwrap_or_default();
    native.item.setKeyEquivalent(&NSString::from_str(key));
    native.item.setKeyEquivalentModifierMask(
        item.accelerator
            .map(accelerator_modifiers)
            .unwrap_or_else(NSEventModifierFlags::empty),
    );

    let symbol = item
        .icon
        .and_then(|icon| icon.name_for(TrayPlatform::Macos));
    let image = symbol.and_then(|symbol| {
        if let Some(image) = controller.ivars().symbol_images.borrow().get(symbol) {
            return Some(image.clone());
        }
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            None,
        )?;
        controller
            .ivars()
            .symbol_images
            .borrow_mut()
            .insert(symbol, image.clone());
        Some(image)
    });
    native.item.setImage(image.as_deref());

    if item.children.is_empty() {
        if native.submenu.take().is_some() {
            native.item.setSubmenu(None);
            native.children.clear();
        }
        if let Some(command) = &item.command {
            let tag = *next_tag;
            *next_tag += 1;
            command_by_tag.insert(tag, command.clone());
            native.item.setTag(tag);
            unsafe {
                native.item.setTarget(Some(controller));
                native.item.setAction(Some(sel!(performTrayCommand:)));
            }
        } else {
            native.item.setTag(0);
            unsafe {
                native.item.setTarget(None);
                native.item.setAction(None);
            }
        }
    } else {
        native.item.setTag(0);
        unsafe {
            native.item.setTarget(None);
            native.item.setAction(None);
        }
        if native.submenu.is_none() {
            let mtm = MainThreadMarker::from(&*native.item);
            let submenu = NSMenu::new(mtm);
            submenu.setAutoenablesItems(false);
            native.item.setSubmenu(Some(&submenu));
            native.submenu = Some(submenu);
        }
        let submenu = native.submenu.as_ref().expect("submenu was created");
        reconcile_items(
            submenu,
            &mut native.children,
            &item.children,
            controller,
            next_tag,
            command_by_tag,
        );
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
    pixels.validate().map_err(|error| anyhow::anyhow!(error))?;
    if pixels.format != crate::PixelFormat::GrayAlpha8 {
        anyhow::bail!("macOS template tray icon must use GrayAlpha8 pixels");
    }
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            mtm.alloc(),
            ptr::null_mut(),
            pixels.width as isize,
            pixels.height as isize,
            8,
            2,
            true,
            false,
            objc2_app_kit::NSDeviceWhiteColorSpace,
            NSBitmapFormat::AlphaNonpremultiplied,
            pixels.stride as isize,
            16,
        )
    }
    .context("create macOS tray bitmap")?;
    unsafe {
        ptr::copy_nonoverlapping(pixels.data.as_ptr(), bitmap.bitmapData(), pixels.data.len());
    }
    let logical_size = NSSize::new(pixels.logical_width as f64, pixels.logical_height as f64);
    bitmap.setSize(logical_size);
    let image = NSImage::initWithSize(mtm.alloc(), logical_size);
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
