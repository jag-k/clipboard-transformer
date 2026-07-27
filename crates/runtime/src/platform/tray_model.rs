use super::{
    one_line, tray_info_lines, AutostartStatus, MenuAccelerator, TrayAction, TrayIcon, TrayLabel,
    TrayMenu, TrayMenuEntry, TrayMenuItem, TrayPlatform, TraySnapshot, TrayText,
};

pub fn build_menu_model(snapshot: &TraySnapshot) -> TrayMenu {
    let mut items = Vec::new();

    if !snapshot.recent.is_empty() {
        items.push(entry(TrayMenuEntry::informational(
            "recent-header",
            "Recent",
        )));
        for recent in &snapshot.recent {
            let relative_time = ct_i18n::relative_time(recent.transformed_at);
            let secondary = recent_secondary_label(recent, &relative_time);
            let mut submenu = TrayMenuEntry::item(
                format!("recent:{}", recent.transform_id),
                TrayLabel::new(recent.result.clone())
                    .subtitle(secondary)
                    .single_line(format!("{} — {relative_time}", recent.result)),
            );
            submenu.children.push(entry(TrayMenuEntry::informational(
                format!("recent:{}:time", recent.transform_id),
                ct_i18n::absolute_time(recent.transformed_at),
            )));
            submenu.children.push(entry(TrayMenuEntry::informational(
                format!("recent:{}:rules-header", recent.transform_id),
                "Rules",
            )));
            for (rule_index, rule) in recent.rules.iter().enumerate() {
                let rule_prefix = format!("recent:{}:rule:{rule_index}", recent.transform_id);
                let mut rule_menu = TrayMenuEntry::item(rule_prefix.clone(), rule.label.as_str());
                rule_menu.children.push(entry(TrayMenuEntry::action(
                    format!("{rule_prefix}:edit"),
                    "Edit rule",
                    TrayAction::EditRule {
                        rule_id: Some(rule.id.clone()),
                    },
                    None,
                )));
                if snapshot.disable_for > 0 {
                    let mut disable = TrayMenuEntry::action(
                        format!("{rule_prefix}:disable"),
                        format!(
                            "Disable for {}",
                            ct_i18n::human_duration(std::time::Duration::from_secs(
                                snapshot.disable_for,
                            ))
                        ),
                        TrayAction::DisableRule {
                            rule_id: rule.id.clone(),
                            seconds: snapshot.disable_for,
                        },
                        None,
                    );
                    disable.icon = Some(
                        TrayIcon::new()
                            .with_macos_sf_symbol("nosign")
                            .with_linux_icon_name("action-unavailable-symbolic"),
                    );
                    rule_menu.children.push(entry(disable));
                }
                submenu.children.push(entry(rule_menu));
            }
            submenu.children.push(TrayMenuItem::Separator);
            let mut undo = TrayMenuEntry::action(
                format!("recent:{}:undo", recent.transform_id),
                "Undo",
                TrayAction::RestoreHistory {
                    transform_id: recent.transform_id,
                },
                None,
            );
            undo.enabled = recent.can_undo;
            submenu.children.push(entry(undo));
            items.push(entry(submenu));
        }
        items.push(TrayMenuItem::Separator);
    }

    items.push(entry(TrayMenuEntry::informational("info-header", "Info")));
    for (index, line) in tray_info_lines(snapshot).into_iter().enumerate() {
        items.push(entry(TrayMenuEntry::informational(
            format!("info:{index}"),
            line,
        )));
    }
    items.push(TrayMenuItem::Separator);

    if !snapshot.plugins.is_empty() {
        let attention_count = snapshot
            .plugins
            .iter()
            .filter(|plugin| plugin.requires_attention)
            .count();
        let header = if attention_count > 0 {
            format!(
                "Plugins ({}, {attention_count} need attention)",
                snapshot.plugins.len()
            )
        } else {
            format!("Plugins ({})", snapshot.plugins.len())
        };
        items.push(entry(TrayMenuEntry::informational(
            "plugins-header",
            header,
        )));
        for plugin in &snapshot.plugins {
            let prefix = format!("plugin:{}", plugin.id);
            let mut submenu = TrayMenuEntry::item(
                prefix.clone(),
                format!("{} — {}", plugin.name, plugin.state),
            );
            for (index, issue) in plugin.issues.iter().take(5).enumerate() {
                submenu.children.push(entry(TrayMenuEntry::informational(
                    format!("{prefix}:issue:{index}"),
                    one_line(issue, 100),
                )));
            }
            if !plugin.issues.is_empty() {
                submenu.children.push(TrayMenuItem::Separator);
            }
            submenu.children.push(entry(TrayMenuEntry::action(
                format!("{prefix}:copy-id"),
                "Copy plugin id",
                TrayAction::CopyText {
                    text: plugin.id.clone(),
                },
                None,
            )));
            submenu.children.push(entry(TrayMenuEntry::action(
                format!("{prefix}:copy-example"),
                "Copy config example command",
                TrayAction::CopyText {
                    text: format!("clipboard-transformer plugin example {}", plugin.id),
                },
                None,
            )));
            submenu.children.push(entry(TrayMenuEntry::action(
                format!("{prefix}:copy-doctor"),
                "Copy diagnostics command",
                TrayAction::CopyText {
                    text: format!("clipboard-transformer plugin doctor {}", plugin.id),
                },
                None,
            )));
            submenu.children.push(entry(TrayMenuEntry::action(
                format!("{prefix}:reveal"),
                TrayText::new("Show module in file manager")
                    .with_platform(TrayPlatform::Macos, "Show module in Finder")
                    .with_platform(TrayPlatform::Windows, "Show module in Explorer"),
                TrayAction::RevealPath {
                    path: plugin.module_path.clone(),
                },
                None,
            )));
            items.push(entry(submenu));
        }
        items.push(TrayMenuItem::Separator);
    }

    let mut pause = TrayMenuEntry::action(
        "pause",
        "Pause Transformations",
        TrayAction::SetPaused(!snapshot.paused),
        None,
    );
    pause.checked = Some(snapshot.paused);
    items.push(entry(pause));
    items.push(TrayMenuItem::Separator);

    if snapshot.autostart != AutostartStatus::Unsupported {
        let (enabled, checked) = match &snapshot.autostart {
            AutostartStatus::Disabled => (true, false),
            AutostartStatus::Enabled => (true, true),
            AutostartStatus::Error(_) => (false, false),
            AutostartStatus::Unsupported => unreachable!(),
        };
        let mut autostart = TrayMenuEntry::action(
            "autostart",
            "Run on Startup",
            TrayAction::SetAutostart(!checked),
            None,
        );
        autostart.enabled = enabled;
        autostart.checked = Some(checked);
        items.push(entry(autostart));
        items.push(TrayMenuItem::Separator);
    }

    items.extend(action_items(snapshot));
    TrayMenu { items }
}

fn recent_secondary_label(recent: &super::TrayRecentItem, relative_time: &str) -> String {
    let rule_context = match recent.rules.as_slice() {
        [] => return relative_time.to_string(),
        [rule] if !rule.label.trim().is_empty() => rule.label.as_str(),
        [rule] => rule.id.as_str(),
        rules => return format!("{} Rules • {relative_time}", rules.len()),
    };
    format!("{rule_context} • {relative_time}")
}

type ActionItemSpec = (
    &'static str,
    TrayLabel,
    TrayAction,
    Option<MenuAccelerator>,
    Option<TrayIcon>,
);

fn action_items(snapshot: &TraySnapshot) -> Vec<TrayMenuItem> {
    let actions: [ActionItemSpec; 6] = [
        (
            "reload",
            "Reload config".into(),
            TrayAction::ReloadConfig,
            Some(MenuAccelerator::Reload),
            Some(
                TrayIcon::new()
                    .with_macos_sf_symbol("arrow.clockwise")
                    .with_linux_icon_name("view-refresh-symbolic"),
            ),
        ),
        (
            "open-config",
            "Open config file".into(),
            TrayAction::OpenConfig,
            Some(MenuAccelerator::Open),
            Some(
                TrayIcon::new()
                    .with_macos_sf_symbol("arrow.up.forward.app")
                    .with_linux_icon_name("document-open-symbolic"),
            ),
        ),
        (
            "reveal-config",
            TrayText::new("Show config in file manager")
                .with_platform(TrayPlatform::Macos, "Show config in Finder")
                .with_platform(TrayPlatform::Windows, "Show config in Explorer")
                .into(),
            TrayAction::RevealConfig,
            Some(MenuAccelerator::Reveal),
            Some(
                TrayIcon::new()
                    .with_macos_sf_symbol("folder")
                    .with_linux_icon_name("folder-open-symbolic"),
            ),
        ),
        (
            "copy-config-path",
            "Copy config path".into(),
            TrayAction::CopyConfigPath,
            Some(MenuAccelerator::Copy),
            None,
        ),
        (
            "clear-history",
            "Clear History".into(),
            TrayAction::ClearHistory,
            None,
            None,
        ),
        (
            "quit",
            "Quit Clipboard Transformer".into(),
            TrayAction::Quit,
            Some(MenuAccelerator::Quit),
            Some(
                TrayIcon::new()
                    .with_macos_sf_symbol("xmark.circle")
                    .with_linux_icon_name("application-exit-symbolic"),
            ),
        ),
    ];
    actions
        .into_iter()
        .map(|(id, label, command, accelerator, icon)| {
            let enabled = !matches!(
                &command,
                TrayAction::OpenConfig | TrayAction::RevealConfig | TrayAction::CopyConfigPath
            ) || snapshot.config_path.is_some();
            let mut item = TrayMenuEntry::action(id, label, command, accelerator);
            item.enabled = enabled;
            item.icon = icon;
            entry(item)
        })
        .collect()
}

fn entry(item: TrayMenuEntry) -> TrayMenuItem {
    TrayMenuItem::Entry(Box::new(item))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    use uuid::Uuid;

    use super::*;
    use crate::config::ConfigWarning;
    use crate::platform::tray::{TrayRecentItem, TrayRule};

    fn snapshot() -> TraySnapshot {
        TraySnapshot {
            recent: Vec::new(),
            rule_count: 2,
            source_count: 1,
            reload_error: None,
            config_warnings: Vec::<ConfigWarning>::new(),
            plugins: Vec::new(),
            config_path: Some(PathBuf::from("/tmp/config.yaml")),
            disable_for: 60,
            autostart: AutostartStatus::Disabled,
            paused: false,
        }
    }

    fn find_entry<'a>(items: &'a [TrayMenuItem], id: &str) -> &'a TrayMenuEntry {
        items
            .iter()
            .find_map(|item| match item {
                TrayMenuItem::Entry(entry) if entry.id == id => Some(entry),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing tray menu entry {id}"))
    }

    #[test]
    fn detailed_labels_keep_an_explicit_single_line_fallback() {
        let label: TrayLabel = TrayLabel::new("Primary")
            .subtitle("Different secondary")
            .single_line("Compact fallback")
            .into();
        assert_eq!(label.title_for(TrayPlatform::Other), "Primary");
        assert_eq!(
            label.subtitle_for(TrayPlatform::Other),
            Some("Different secondary")
        );
        assert_eq!(
            label.single_line_for(TrayPlatform::Linux),
            "Compact fallback"
        );
    }

    #[test]
    fn labels_accept_strings_and_resolve_missing_fallback_to_title() {
        let plain = TrayMenuEntry::item("plain", "Plain title");
        assert_eq!(
            plain.label.single_line_for(TrayPlatform::Windows),
            "Plain title"
        );
        assert!(!plain.label.has_subtitle());

        let detailed = TrayMenuEntry::item(
            "detailed",
            TrayLabel::new("Detailed title").subtitle("Optional subtitle"),
        );
        assert_eq!(
            detailed.label.title_for(TrayPlatform::Macos),
            "Detailed title"
        );
        assert_eq!(
            detailed.label.subtitle_for(TrayPlatform::Macos),
            Some("Optional subtitle")
        );
        assert_eq!(
            detailed.label.single_line_for(TrayPlatform::Windows),
            "Detailed title"
        );

        let relabeled = detailed.label(String::from("Replacement"));
        assert_eq!(
            relabeled.label.single_line_for(TrayPlatform::Linux),
            "Replacement"
        );
    }

    #[test]
    fn icons_are_independent_and_optional_per_platform() {
        let icon = TrayIcon::new()
            .with_macos_sf_symbol("arrow.clockwise")
            .with_linux_icon_name("view-refresh-symbolic");
        assert_eq!(icon.name_for(TrayPlatform::Macos), Some("arrow.clockwise"));
        assert_eq!(icon.name_for(TrayPlatform::Windows), None);
        assert_eq!(
            icon.name_for(TrayPlatform::Linux),
            Some("view-refresh-symbolic")
        );

        let windows_icon = TrayIcon::new().with_windows_resource("IDB_REFRESH");
        assert_eq!(
            windows_icon.name_for(TrayPlatform::Windows),
            Some("IDB_REFRESH")
        );
        assert_eq!(windows_icon.name_for(TrayPlatform::Macos), None);
    }

    #[test]
    fn recent_entry_exposes_two_line_content_and_legacy_fallback() {
        let mut snapshot = snapshot();
        let transformed_at = SystemTime::now() - Duration::from_secs(60);
        let transform_id = Uuid::new_v4();
        snapshot.recent.push(TrayRecentItem {
            transform_id,
            result: "Transformed result".into(),
            transformed_at,
            rules: vec![TrayRule {
                id: "rule".into(),
                label: "Rule".into(),
            }],
            can_undo: true,
        });

        let menu = build_menu_model(&snapshot);
        let recent = find_entry(&menu.items, &format!("recent:{transform_id}"));
        assert_eq!(
            recent.label.title_for(TrayPlatform::Other),
            "Transformed result"
        );
        assert!(recent
            .label
            .subtitle_for(TrayPlatform::Macos)
            .unwrap()
            .starts_with("Rule • "));
        assert!(recent
            .label
            .single_line_for(TrayPlatform::Other)
            .starts_with("Transformed result — "));
    }

    #[test]
    fn recent_secondary_uses_rule_count_or_single_rule_identity() {
        let recent = TrayRecentItem {
            transform_id: Uuid::new_v4(),
            result: "Result".into(),
            transformed_at: SystemTime::now(),
            rules: vec![TrayRule {
                id: "fallback-id".into(),
                label: "Named rule".into(),
            }],
            can_undo: true,
        };
        assert_eq!(
            recent_secondary_label(&recent, "2 minutes ago"),
            "Named rule • 2 minutes ago"
        );

        let mut id_fallback = recent.clone();
        id_fallback.rules[0].label.clear();
        assert_eq!(
            recent_secondary_label(&id_fallback, "2 minutes ago"),
            "fallback-id • 2 minutes ago"
        );

        let mut multiple = recent;
        multiple.rules.push(TrayRule {
            id: "second".into(),
            label: "Second rule".into(),
        });
        multiple.rules.push(TrayRule {
            id: "third".into(),
            label: "Third rule".into(),
        });
        assert_eq!(
            recent_secondary_label(&multiple, "2 minutes ago"),
            "3 Rules • 2 minutes ago"
        );
    }

    #[test]
    fn action_model_keeps_commands_accelerators_and_icons() {
        let menu = build_menu_model(&snapshot());
        let reload = find_entry(&menu.items, "reload");
        assert_eq!(reload.command, Some(TrayAction::ReloadConfig));
        assert_eq!(reload.accelerator, Some(MenuAccelerator::Reload));
        assert_eq!(
            reload.icon.unwrap().macos_sf_symbol,
            Some("arrow.clockwise")
        );

        let quit = find_entry(&menu.items, "quit");
        assert_eq!(quit.command, Some(TrayAction::Quit));
        assert_eq!(quit.accelerator, Some(MenuAccelerator::Quit));
    }

    #[test]
    fn model_exposes_checked_and_enabled_state_without_backend_types() {
        let mut snapshot = snapshot();
        snapshot.paused = true;
        snapshot.config_path = None;
        snapshot.autostart = AutostartStatus::Error("unavailable".into());

        let menu = build_menu_model(&snapshot);
        let pause = find_entry(&menu.items, "pause");
        assert_eq!(pause.checked, Some(true));
        assert_eq!(pause.command, Some(TrayAction::SetPaused(false)));

        let autostart = find_entry(&menu.items, "autostart");
        assert!(!autostart.enabled);
        assert_eq!(autostart.checked, Some(false));

        assert!(!find_entry(&menu.items, "open-config").enabled);
        assert!(!find_entry(&menu.items, "reveal-config").enabled);
        assert!(!find_entry(&menu.items, "copy-config-path").enabled);
    }
}
