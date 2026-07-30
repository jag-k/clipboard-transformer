//! Semantic tray menu model.
//!
//! This crate describes menu *shape* — labels, icons, accelerators, nesting —
//! and carries a [`TrayAction`] on the entries the user can activate. Deciding
//! what an action *does* stays with the host, which converts it into its own
//! command at the [`ActionSink`] it supplies.

use std::marker::PhantomData;
use std::path::PathBuf;

#[cfg(feature = "native")]
pub mod native;

/// What the user chose in the tray menu.
///
/// A concrete enum rather than the host's own command type, for the same reason
/// as `ct_notifications::NotificationAction`: `objc2`'s `define_class!` cannot
/// be generic, and the macOS backend keeps its callback and tag map in class
/// ivars. The model was generic over the command type until the tray turned out
/// to have exactly one host — a browser host gets no tray — so the generic only
/// bought a second spelling of every menu type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    RestoreHistory { transform_id: uuid::Uuid },
    EditRule { rule_id: Option<String> },
    DisableRule { rule_id: String, seconds: u64 },
    ReloadConfig,
    OpenConfig,
    RevealConfig,
    CopyConfigPath,
    CopyText { text: String },
    RevealPath { path: PathBuf },
    ClearHistory,
    SetAutostart(bool),
    SetPaused(bool),
    Quit,
}

/// Where native backends deliver chosen actions. A closure, so the host can
/// convert into its own command type and push onto the single channel it
/// already drains — no adapter thread, no second channel.
pub type ActionSink = Box<dyn Fn(TrayAction) + Send>;

/// Produces the current menu on demand. Backends call it when the menu is about
/// to open, so relative timestamps are formatted at open time instead of
/// drifting until the next host update.
#[cfg(feature = "native")]
pub type TrayMenuSource = Box<dyn Fn() -> TrayMenu + Send>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PixelFormat {
    Rgba8,
    GrayAlpha8,
}

impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgba8 => 4,
            Self::GrayAlpha8 => 2,
        }
    }
}

/// Pixel buffer for a rendered tray icon, including its exact byte layout.
#[derive(Clone, Copy, Debug)]
pub struct TrayIconPixels {
    /// Dimensions of the encoded pixel payload.
    pub width: u32,
    pub height: u32,
    /// Dimensions in platform-independent display units.
    pub logical_width: u32,
    pub logical_height: u32,
    pub format: PixelFormat,
    pub stride: u32,
    pub data: &'static [u8],
}

impl TrayIconPixels {
    pub fn validate(self) -> Result<(), &'static str> {
        if self.logical_width == 0 || self.logical_height == 0 {
            return Err("tray icon logical dimensions must be non-zero");
        }
        let minimum_stride = self
            .width
            .checked_mul(self.format.bytes_per_pixel())
            .ok_or("tray icon row size overflows")?;
        if self.stride < minimum_stride {
            return Err("tray icon stride is shorter than one row");
        }
        let expected = self
            .stride
            .checked_mul(self.height)
            .ok_or("tray icon payload size overflows")? as usize;
        if self.data.len() != expected {
            return Err("tray icon payload length does not match dimensions and stride");
        }
        Ok(())
    }

    pub fn rgba8(self) -> Option<&'static [u8]> {
        (self.format == PixelFormat::Rgba8).then_some(self.data)
    }
}

include!("native/icons.rs");

pub fn macos_template_icon() -> TrayIconPixels {
    MACOS_TEMPLATE
}

pub fn themed_icon(dark_theme: bool) -> TrayIconPixels {
    if dark_theme {
        DARK_THEME
    } else {
        LIGHT_THEME
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrayPlatform {
    Macos,
    Windows,
    Linux,
    Other,
}

impl TrayPlatform {
    pub const fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::Macos
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Self::Other
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAccelerator {
    Reload,
    Open,
    Reveal,
    Copy,
    Quit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcceleratorKey {
    C,
    O,
    Q,
    R,
    F4,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcceleratorModel {
    pub command: bool,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: AcceleratorKey,
}

pub fn accelerator_model(accelerator: MenuAccelerator) -> AcceleratorModel {
    match accelerator {
        MenuAccelerator::Reload => AcceleratorModel {
            command: cfg!(target_os = "macos"),
            control: !cfg!(target_os = "macos"),
            alt: false,
            shift: false,
            key: AcceleratorKey::R,
        },
        MenuAccelerator::Open => AcceleratorModel {
            command: cfg!(target_os = "macos"),
            control: !cfg!(target_os = "macos"),
            alt: false,
            shift: false,
            key: AcceleratorKey::O,
        },
        MenuAccelerator::Reveal => AcceleratorModel {
            command: cfg!(target_os = "macos"),
            control: !cfg!(target_os = "macos"),
            alt: false,
            shift: true,
            key: AcceleratorKey::O,
        },
        MenuAccelerator::Copy => AcceleratorModel {
            command: cfg!(target_os = "macos"),
            control: !cfg!(target_os = "macos"),
            alt: false,
            shift: false,
            key: AcceleratorKey::C,
        },
        MenuAccelerator::Quit => {
            #[cfg(target_os = "macos")]
            {
                AcceleratorModel {
                    command: true,
                    control: false,
                    alt: false,
                    shift: false,
                    key: AcceleratorKey::Q,
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                AcceleratorModel {
                    command: false,
                    control: false,
                    alt: true,
                    shift: false,
                    key: AcceleratorKey::F4,
                }
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayText {
    pub default: String,
    pub macos: Option<String>,
    pub windows: Option<String>,
    pub linux: Option<String>,
}

impl TrayText {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            default: value.into(),
            macos: None,
            windows: None,
            linux: None,
        }
    }

    pub fn with_platform(mut self, platform: TrayPlatform, value: impl Into<String>) -> Self {
        let value = Some(value.into());
        match platform {
            TrayPlatform::Macos => self.macos = value,
            TrayPlatform::Windows => self.windows = value,
            TrayPlatform::Linux => self.linux = value,
            TrayPlatform::Other => {}
        }
        self
    }

    pub fn for_platform(&self, platform: TrayPlatform) -> &str {
        match platform {
            TrayPlatform::Macos => self.macos.as_deref(),
            TrayPlatform::Windows => self.windows.as_deref(),
            TrayPlatform::Linux => self.linux.as_deref(),
            TrayPlatform::Other => None,
        }
        .unwrap_or(&self.default)
    }
}

impl From<&str> for TrayText {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for TrayText {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// A tray item label with a required title and optional richer presentation.
///
/// Plain strings convert into a title-only label. A label created with
/// [`TrayLabel::new`] can additionally provide a subtitle and, after that, an
/// explicit single-line fallback:
///
/// ```
/// use ct_tray::TrayLabel;
///
/// let title_only = TrayLabel::new("Reload");
/// let detailed = TrayLabel::new("github.com/jag-k/clipboard-transformer")
///     .subtitle("3 Rules • 2 minutes ago")
///     .single_line("github.com/jag-k/clipboard-transformer — 2 minutes ago");
/// ```
///
/// Resolution is controlled by the backend:
///
/// - a backend supporting subtitles renders `title` and `subtitle`;
/// - a single-line backend renders `single_line`, when supplied;
/// - otherwise a single-line backend renders `title`.
///
/// Each component is a [`TrayText`], so its platform override is resolved
/// before the backend chooses the two-line or single-line representation.
///
/// `single_line` is only available after [`TrayLabelBuilder::subtitle`].
/// Without a subtitle it would compete with the required title while still
/// producing only one line, making it unclear which value is canonical.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayLabel {
    title: TrayText,
    subtitle: Option<TrayText>,
    single_line: Option<TrayText>,
}

impl TrayLabel {
    /// Starts a label with its required title.
    ///
    /// `single_line` is intentionally unavailable until `subtitle` has been
    /// supplied. Passing this builder directly to `TrayMenuEntry::label` or
    /// another `impl Into<TrayLabel>` boundary resolves a title-only label.
    ///
    /// ```compile_fail
    /// use ct_tray::TrayLabel;
    ///
    /// let _ = TrayLabel::new("Title").single_line("Compact");
    /// ```
    #[allow(clippy::new_ret_no_self)] // Typestate intentionally starts in a builder.
    pub fn new(title: impl Into<TrayText>) -> TrayLabelBuilder<NoSubtitle> {
        TrayLabelBuilder {
            title: title.into(),
            subtitle: None,
            single_line: None,
            state: PhantomData,
        }
    }

    pub fn title_for(&self, platform: TrayPlatform) -> &str {
        self.title.for_platform(platform)
    }

    pub fn subtitle_for(&self, platform: TrayPlatform) -> Option<&str> {
        self.subtitle
            .as_ref()
            .map(|value| value.for_platform(platform))
    }

    pub fn single_line_for(&self, platform: TrayPlatform) -> &str {
        self.single_line
            .as_ref()
            .unwrap_or(&self.title)
            .for_platform(platform)
    }

    pub fn has_subtitle(&self) -> bool {
        self.subtitle.is_some()
    }
}

/// Typestate used by `TrayLabel::new`; users normally do not name this type.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NoSubtitle;

/// Typestate used after `TrayLabelBuilder::subtitle`.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WithSubtitle;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayLabelBuilder<State> {
    title: TrayText,
    subtitle: Option<TrayText>,
    single_line: Option<TrayText>,
    state: PhantomData<State>,
}

impl TrayLabelBuilder<NoSubtitle> {
    pub fn subtitle(self, subtitle: impl Into<TrayText>) -> TrayLabelBuilder<WithSubtitle> {
        TrayLabelBuilder {
            title: self.title,
            subtitle: Some(subtitle.into()),
            single_line: None,
            state: PhantomData,
        }
    }
}

impl TrayLabelBuilder<WithSubtitle> {
    pub fn single_line(mut self, single_line: impl Into<TrayText>) -> Self {
        self.single_line = Some(single_line.into());
        self
    }
}

impl From<TrayLabelBuilder<NoSubtitle>> for TrayLabel {
    fn from(builder: TrayLabelBuilder<NoSubtitle>) -> Self {
        Self {
            title: builder.title,
            subtitle: None,
            single_line: None,
        }
    }
}

impl From<TrayLabelBuilder<WithSubtitle>> for TrayLabel {
    fn from(builder: TrayLabelBuilder<WithSubtitle>) -> Self {
        Self {
            title: builder.title,
            subtitle: builder.subtitle,
            single_line: builder.single_line,
        }
    }
}

impl From<TrayText> for TrayLabel {
    fn from(title: TrayText) -> Self {
        Self {
            title,
            subtitle: None,
            single_line: None,
        }
    }
}

impl From<&str> for TrayLabel {
    fn from(title: &str) -> Self {
        TrayText::from(title).into()
    }
}

impl From<String> for TrayLabel {
    fn from(title: String) -> Self {
        TrayText::from(title).into()
    }
}

/// Platform-specific icon names carried without conditional compilation.
///
/// A missing name means that platform renders a normal text-only item.
/// Backends also fall back to text-only when they cannot render a supplied
/// name. No backend substitutes an icon from another platform.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TrayIcon {
    pub macos_sf_symbol: Option<&'static str>,
    pub windows_resource: Option<&'static str>,
    pub linux_icon_name: Option<&'static str>,
}

impl TrayIcon {
    pub const fn new() -> Self {
        Self {
            macos_sf_symbol: None,
            windows_resource: None,
            linux_icon_name: None,
        }
    }

    pub const fn with_macos_sf_symbol(mut self, name: &'static str) -> Self {
        self.macos_sf_symbol = Some(name);
        self
    }

    /// Adds the name of a bitmap resource linked into the Windows executable.
    pub const fn with_windows_resource(mut self, name: &'static str) -> Self {
        self.windows_resource = Some(name);
        self
    }

    pub const fn with_linux_icon_name(mut self, name: &'static str) -> Self {
        self.linux_icon_name = Some(name);
        self
    }

    pub const fn name_for(self, platform: TrayPlatform) -> Option<&'static str> {
        match platform {
            TrayPlatform::Macos => self.macos_sf_symbol,
            TrayPlatform::Windows => self.windows_resource,
            TrayPlatform::Linux => self.linux_icon_name,
            TrayPlatform::Other => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrayMenuItem {
    Entry(Box<TrayMenuEntry>),
    Separator,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayMenuEntry {
    pub id: String,
    pub label: TrayLabel,
    pub enabled: bool,
    pub visible: bool,
    pub checked: Option<bool>,
    pub icon: Option<TrayIcon>,
    pub accelerator: Option<MenuAccelerator>,
    pub command: Option<TrayAction>,
    pub children: Vec<TrayMenuItem>,
}

impl TrayMenuEntry {
    pub fn item(id: impl Into<String>, label: impl Into<TrayLabel>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            enabled: true,
            visible: true,
            checked: None,
            icon: None,
            accelerator: None,
            command: None,
            children: Vec::new(),
        }
    }

    pub fn informational(id: impl Into<String>, label: impl Into<TrayLabel>) -> Self {
        let mut item = Self::item(id, label);
        item.enabled = false;
        item
    }

    pub fn action(
        id: impl Into<String>,
        label: impl Into<TrayLabel>,
        command: TrayAction,
        accelerator: Option<MenuAccelerator>,
    ) -> Self {
        let mut item = Self::item(id, label);
        item.command = Some(command);
        item.accelerator = accelerator;
        item
    }

    /// Replaces this entry's label.
    ///
    /// This accepts anything convertible into [`TrayLabel`]. Pass `&str`,
    /// [`String`], or [`TrayText`] for a title-only label without constructing
    /// a `TrayLabel` explicitly. Pass the builder returned by
    /// [`TrayLabel::new`] for an optional subtitle and single-line fallback.
    ///
    /// At rendering time, title-only values resolve to their title everywhere.
    /// Detailed values resolve to `title` plus `subtitle` on a backend that
    /// supports subtitles, and to `single_line` or, if it was omitted, `title`
    /// on a single-line backend.
    pub fn label(mut self, label: impl Into<TrayLabel>) -> Self {
        self.label = label.into();
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayMenu {
    pub items: Vec<TrayMenuItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_model_nests_entries_separators_and_actions() {
        let mut root = TrayMenu { items: Vec::new() };
        root.items
            .push(TrayMenuItem::Entry(Box::new(TrayMenuEntry::informational(
                "header", "Section",
            ))));
        root.items.push(TrayMenuItem::Separator);

        let mut parent = TrayMenuEntry::item("parent", "Parent");
        parent
            .children
            .push(TrayMenuItem::Entry(Box::new(TrayMenuEntry::action(
                "child",
                "Child",
                TrayAction::CopyConfigPath,
                Some(MenuAccelerator::Copy),
            ))));
        root.items.push(TrayMenuItem::Entry(Box::new(parent)));

        let TrayMenuItem::Entry(header) = &root.items[0] else {
            panic!("expected an entry");
        };
        assert!(!header.enabled, "informational entries are not clickable");
        assert_eq!(header.command, None);

        assert!(matches!(root.items[1], TrayMenuItem::Separator));

        let TrayMenuItem::Entry(parent) = &root.items[2] else {
            panic!("expected an entry");
        };
        assert!(parent.enabled);
        let TrayMenuItem::Entry(child) = &parent.children[0] else {
            panic!("expected a child entry");
        };
        assert_eq!(child.command, Some(TrayAction::CopyConfigPath));
        assert_eq!(child.accelerator, Some(MenuAccelerator::Copy));
    }

    #[test]
    fn action_entries_carry_their_command_and_plain_items_do_not() {
        let action = TrayMenuEntry::action("act", "Act", TrayAction::ReloadConfig, None);
        assert_eq!(action.command, Some(TrayAction::ReloadConfig));
        assert!(action.enabled);
        assert_eq!(action.accelerator, None);

        let plain = TrayMenuEntry::item("plain", "Plain");
        assert_eq!(plain.command, None);
        assert!(plain.visible);
        assert_eq!(plain.checked, None);
    }

    #[test]
    fn a_title_only_label_resolves_to_its_title_on_every_platform() {
        let entry = TrayMenuEntry::item("id", "Reload");
        for platform in [
            TrayPlatform::Macos,
            TrayPlatform::Windows,
            TrayPlatform::Linux,
            TrayPlatform::Other,
        ] {
            assert_eq!(entry.label.title_for(platform), "Reload");
            assert_eq!(entry.label.subtitle_for(platform), None);
            // Without an explicit single-line value the title is the fallback.
            assert_eq!(entry.label.single_line_for(platform), "Reload");
        }
        assert!(!entry.label.has_subtitle());
    }

    #[test]
    fn a_detailed_label_keeps_subtitle_and_single_line_separate() {
        let label: TrayLabel = TrayLabel::new("Result")
            .subtitle("2 seconds ago")
            .single_line("Result — 2 seconds ago")
            .into();

        assert!(label.has_subtitle());
        assert_eq!(label.title_for(TrayPlatform::Macos), "Result");
        assert_eq!(
            label.subtitle_for(TrayPlatform::Macos),
            Some("2 seconds ago")
        );
        assert_eq!(
            label.single_line_for(TrayPlatform::Linux),
            "Result — 2 seconds ago"
        );
    }

    #[test]
    fn a_subtitle_without_a_single_line_value_falls_back_to_the_title() {
        let label: TrayLabel = TrayLabel::new("Title").subtitle("Detail").into();

        assert_eq!(label.single_line_for(TrayPlatform::Windows), "Title");
        assert_eq!(label.subtitle_for(TrayPlatform::Windows), Some("Detail"));
    }

    #[test]
    fn platform_overrides_apply_only_to_the_named_platform() {
        let text = TrayText::new("Preferences").with_platform(TrayPlatform::Windows, "Settings");

        assert_eq!(text.for_platform(TrayPlatform::Windows), "Settings");
        assert_eq!(text.for_platform(TrayPlatform::Macos), "Preferences");
        assert_eq!(text.for_platform(TrayPlatform::Linux), "Preferences");
        assert_eq!(text.for_platform(TrayPlatform::Other), "Preferences");
    }

    #[test]
    fn every_accelerator_uses_the_host_platform_convention() {
        // Command on macOS, Control elsewhere, for the shortcuts that share a
        // key with the platform's own convention.
        for accelerator in [
            MenuAccelerator::Reload,
            MenuAccelerator::Open,
            MenuAccelerator::Reveal,
            MenuAccelerator::Copy,
        ] {
            let model = accelerator_model(accelerator);
            assert_eq!(model.command, cfg!(target_os = "macos"));
            assert_eq!(model.control, !cfg!(target_os = "macos"));
            assert!(!model.alt);
        }

        assert!(accelerator_model(MenuAccelerator::Reveal).shift);
        assert!(!accelerator_model(MenuAccelerator::Open).shift);
        assert_eq!(
            accelerator_model(MenuAccelerator::Open).key,
            AcceleratorKey::O
        );
        assert_eq!(
            accelerator_model(MenuAccelerator::Reveal).key,
            AcceleratorKey::O
        );
    }

    #[test]
    fn quit_uses_command_q_on_macos_and_alt_f4_elsewhere() {
        let quit = accelerator_model(MenuAccelerator::Quit);
        #[cfg(target_os = "macos")]
        assert_eq!(
            quit,
            AcceleratorModel {
                command: true,
                control: false,
                alt: false,
                shift: false,
                key: AcceleratorKey::Q,
            }
        );
        #[cfg(not(target_os = "macos"))]
        assert_eq!(
            quit,
            AcceleratorModel {
                command: false,
                control: false,
                alt: true,
                shift: false,
                key: AcceleratorKey::F4,
            }
        );
    }

    #[test]
    fn the_current_platform_is_the_one_this_test_was_compiled_for() {
        let expected = if cfg!(target_os = "macos") {
            TrayPlatform::Macos
        } else if cfg!(target_os = "windows") {
            TrayPlatform::Windows
        } else if cfg!(target_os = "linux") {
            TrayPlatform::Linux
        } else {
            TrayPlatform::Other
        };
        assert_eq!(TrayPlatform::current(), expected);
    }
}
