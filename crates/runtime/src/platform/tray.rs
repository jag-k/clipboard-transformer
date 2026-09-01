use std::path::PathBuf;
use std::time::SystemTime;

use uuid::Uuid;

use crate::config::ConfigWarning;
pub use crate::platform::autostart::AutostartStatus;

#[path = "tray_model.rs"]
mod model;
pub use ct_tray::{
    accelerator_model, AcceleratorKey, AcceleratorModel, MenuAccelerator, NoSubtitle, TrayIcon,
    TrayLabel, TrayLabelBuilder, TrayPlatform, TrayText, WithSubtitle,
};
pub use model::build_menu_model;

pub use ct_tray::{macos_template_icon, themed_icon, TrayAction, TrayIconPixels};

// Composition here builds a `TrayAction` menu; the app converts each action into
// its own `AppCommand` at the sink it hands the tray.
pub use ct_tray::{TrayMenu, TrayMenuEntry, TrayMenuItem};

/// Produces the current menu on demand; defined by `ct-tray`, re-exported here
/// because this module is where the sources are built.
///
/// Native backends hold one of these instead of a [`TraySnapshot`], which is
/// what keeps `TraySnapshot` and [`build_menu_model`] — and therefore the whole
/// application vocabulary — out of `ct-tray`.
///
/// Backends call it when the menu is about to open, not when the host publishes
/// an update, so relative timestamps are formatted at open time. Rendering them
/// eagerly would either drift until the next host update or require a refresh
/// timer that closes an open menu.
pub use ct_tray::TrayMenuSource;

/// Builds a self-contained menu source by taking ownership of `snapshot`.
///
/// Prefer [`TrayStateHandle`]: a source built this way is frozen to one
/// snapshot, so keeping it current would require republishing on a timer.
pub fn menu_source(snapshot: TraySnapshot) -> TrayMenuSource {
    Box::new(move || build_menu_model(&snapshot))
}

/// Tray-visible application state, shared between `Agent` and the menu source.
///
/// `Agent` writes here when the state it exposes to the tray changes; the source
/// reads it when the user opens the menu. Nothing is recomputed on a timer, and
/// the tray is never told to re-read its menu — being told to would close a menu
/// the user has open.
///
/// Only this state is shared, not `Agent`: it is plain data, so it is `Send`,
/// while `Agent` holds platform handles that are not (`MacosNotificationBackend`
/// keeps `Retained<UNUserNotificationCenter>`). A `Mutex` because the source runs
/// on whichever thread owns the native tray — its own on Linux
/// (`ksni::Tray: Sized + Send + 'static`), the main thread elsewhere.
#[derive(Clone, Debug)]
pub struct TrayStateHandle {
    shared: std::sync::Arc<std::sync::Mutex<TraySnapshot>>,
    /// Counts actual writes. Lets callers and tests tell "nothing changed" from
    /// "nobody looked", which is the difference between a correct idle tick and a
    /// republish on a timer.
    generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl TrayStateHandle {
    pub fn new(state: TraySnapshot) -> Self {
        Self {
            shared: std::sync::Arc::new(std::sync::Mutex::new(state)),
            generation: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Number of times the state has actually been replaced.
    pub fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The single source handed to the tray, for the whole process lifetime.
    ///
    /// Builds the menu from current state on every call, so relative timestamps
    /// are formatted when the menu opens rather than when state last changed.
    pub fn source(&self) -> TrayMenuSource {
        let shared = std::sync::Arc::clone(&self.shared);
        Box::new(move || match shared.lock() {
            Ok(state) => build_menu_model(&state),
            // A poisoned lock means a previous menu build panicked. An empty menu
            // beats propagating that into the tray's thread.
            Err(_) => TrayMenu { items: Vec::new() },
        })
    }

    /// Replaces the shared state. Returns whether it differed.
    pub fn store(&self, state: TraySnapshot) -> bool {
        match self.shared.lock() {
            Ok(mut current) if *current != state => {
                *current = state;
                self.generation
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayRule {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayRecentItem {
    pub transform_id: Uuid,
    pub result: String,
    pub transformed_at: SystemTime,
    pub rules: Vec<TrayRule>,
    pub can_undo: bool,
}

/// Host-rendered plugin entry: state, concise issues, and copy/reveal
/// actions. Plugins cannot contribute their own UI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayPlugin {
    pub id: String,
    pub name: String,
    pub state: &'static str,
    pub issues: Vec<String>,
    pub requires_attention: bool,
    pub module_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayGroup {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub rule_count: usize,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraySnapshot {
    pub recent: Vec<TrayRecentItem>,
    pub rule_count: usize,
    pub source_count: usize,
    pub reload_error: Option<String>,
    pub config_warnings: Vec<ConfigWarning>,
    pub plugins: Vec<TrayPlugin>,
    pub config_path: Option<PathBuf>,
    pub disable_for: u64,
    pub autostart: AutostartStatus,
    pub paused: bool,
    pub groups: Vec<TrayGroup>,
    pub group_overflow: usize,
    pub group_state_error: Option<String>,
}

fn tray_info_lines(snapshot: &TraySnapshot) -> Vec<String> {
    let mut lines = vec![if let Some(error) = &snapshot.reload_error {
        format!("Config error: {}", one_line(error, 100))
    } else {
        format!(
            "{} rule(s) loaded from {} file(s)",
            snapshot.rule_count, snapshot.source_count
        )
    }];
    if let Some(error) = &snapshot.group_state_error {
        lines.push(format!("Group state error: {}", one_line(error, 100)));
    }
    lines.extend(
        snapshot
            .config_warnings
            .iter()
            .map(|warning| match warning {
                ConfigWarning::ImportCycle { .. } => format!("Import cycle: {warning}"),
                _ => format!("Warning: {warning}"),
            }),
    );
    lines
}

fn one_line(value: &str, limit: usize) -> String {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = value.chars();
    let shortened = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{shortened}…")
    } else {
        shortened
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tray gets one source for the whole process lifetime, and it must see
    /// snapshots stored after it was created — otherwise the host would have to
    /// republish, which is what closes an open menu.
    #[test]
    fn one_source_observes_snapshots_stored_after_it_was_created() {
        let cell = TrayStateHandle::new(snapshot_with_warnings(Vec::new()));
        let source = cell.source();

        let before = source();
        assert!(!menu_mentions_ignored_plugin_warning(&before));

        assert!(cell.store(snapshot_with_warnings(vec![
            ConfigWarning::IgnoredRuleType {
                kind: "plugin".into(),
            }
        ])));

        let after = source();
        assert!(
            menu_mentions_ignored_plugin_warning(&after),
            "the same source must render the newer snapshot"
        );
    }

    #[test]
    fn storing_an_identical_snapshot_reports_no_change() {
        let snapshot = snapshot_with_warnings(Vec::new());
        let cell = TrayStateHandle::new(snapshot.clone());

        assert!(!cell.store(snapshot.clone()), "identical stores are no-ops");
        assert!(!cell.store(snapshot));
    }

    /// Elapsed time is not a snapshot change, yet the label must still be
    /// current: it is formatted inside the source, at open time.
    #[test]
    fn elapsed_time_needs_no_store_to_stay_correct() {
        let snapshot = snapshot_with_warnings(Vec::new());
        let cell = TrayStateHandle::new(snapshot.clone());
        let source = cell.source();

        std::thread::sleep(std::time::Duration::from_millis(5));

        assert!(!cell.store(snapshot), "time passing is not a change");
        assert!(!source().items.is_empty(), "the source still renders");
    }

    fn menu_mentions_ignored_plugin_warning(menu: &TrayMenu) -> bool {
        menu.items.iter().any(|item| match item {
            TrayMenuItem::Entry(entry) => entry
                .label
                .title_for(TrayPlatform::current())
                .contains("plugin"),
            TrayMenuItem::Separator => false,
        })
    }

    /// The alias must stay `Fn`, and backends must call it per open rather than
    /// caching one result: relative timestamps are formatted inside the source.
    #[test]
    fn a_menu_source_is_invoked_on_every_open() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let source: TrayMenuSource = Box::new(move || {
            counted.fetch_add(1, Ordering::Relaxed);
            build_menu_model(&snapshot_with_warnings(Vec::new()))
        });

        for _ in 0..3 {
            assert!(!source().items.is_empty());
        }
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }

    /// `ksni::Tray: Sized + Send + 'static` owns the Linux tray state on another
    /// thread, so a source that captured non-`Send` state would not compile
    /// there. Assert it here, where every platform's test run sees it.
    #[test]
    fn a_menu_source_built_from_a_snapshot_is_send() {
        fn assert_send<T: Send>(_: T) {}

        assert_send(menu_source(snapshot_with_warnings(Vec::new())));
    }

    /// The source owns its snapshot, so it never borrows application state.
    #[test]
    fn a_menu_source_outlives_the_snapshot_it_was_built_from() {
        let source = {
            let snapshot = snapshot_with_warnings(Vec::new());
            menu_source(snapshot)
        };

        let menu = source();
        assert!(menu
            .items
            .iter()
            .any(|item| matches!(item, TrayMenuItem::Entry(entry) if entry.id == "quit")));
    }

    #[test]
    fn quit_is_a_menu_accelerator_not_a_global_hotkey() {
        let menu = build_menu_model(&snapshot_with_warnings(Vec::new()));
        let quit = menu
            .items
            .iter()
            .find_map(|item| match item {
                TrayMenuItem::Entry(item) if item.id == "quit" => Some(item),
                _ => None,
            })
            .expect("quit menu item");
        assert_eq!(quit.command, Some(TrayAction::Quit));
        assert_eq!(quit.accelerator, Some(MenuAccelerator::Quit));
    }

    #[test]
    fn quit_accelerator_matches_the_platform_convention() {
        let accelerator = accelerator_model(MenuAccelerator::Quit);
        #[cfg(target_os = "macos")]
        assert_eq!(
            accelerator,
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
            accelerator,
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
    fn generated_icons_have_valid_explicit_pixel_layouts() {
        for icon in [macos_template_icon(), themed_icon(false), themed_icon(true)] {
            icon.validate().unwrap();
        }
        assert_eq!(
            macos_template_icon().format,
            ct_tray::PixelFormat::GrayAlpha8
        );
        assert_eq!(themed_icon(false).format, ct_tray::PixelFormat::Rgba8);
        assert_eq!(themed_icon(true).format, ct_tray::PixelFormat::Rgba8);
    }

    fn snapshot_with_warnings(warnings: Vec<ConfigWarning>) -> TraySnapshot {
        TraySnapshot {
            recent: Vec::new(),
            rule_count: 2,
            source_count: 1,
            reload_error: None,
            config_warnings: warnings,
            plugins: Vec::new(),
            config_path: None,
            disable_for: 0,
            autostart: AutostartStatus::Unsupported,
            paused: false,
            groups: Vec::new(),
            group_overflow: 0,
            group_state_error: None,
        }
    }

    #[test]
    fn tray_info_keeps_import_cycles_separate_and_displays_unknown_types() {
        let lines = tray_info_lines(&snapshot_with_warnings(vec![
            ConfigWarning::ImportCycle {
                chain: vec![PathBuf::from("a.yaml"), PathBuf::from("b.yaml")],
            },
            ConfigWarning::IgnoredRuleType {
                kind: "plugin".into(),
            },
            ConfigWarning::InvalidRule {
                id: Some("broken".into()),
                kind: "regexp".into(),
                reason: "regexp rule requires to".into(),
            },
        ]));

        assert!(lines[1].starts_with("Import cycle:"));
        assert_eq!(
            lines[2],
            "Warning: rule type \"plugin\" ignored: no registered handler"
        );
        assert_eq!(
            lines[3],
            "Warning: rule \"broken\" (regexp) ignored: regexp rule requires to"
        );
    }
}
