use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use ct_core::{AppMatcher, AppMode, RawRule};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AppConfig {
    /// Number of recent transformations shown in the tray. Set to 0 to hide the section.
    pub recent_items_count: usize,
    /// Maximum total clipboard representation bytes processed per item. Set to 0 for no limit.
    pub max_item_bytes: u64,
    /// Maximum combined before/after payload bytes retained in history. Set to 0 for no limit.
    pub max_history_bytes: u64,
    /// Persist the latest external clipboard item for the explicit CLI inspect command.
    pub persist_last_clipboard: bool,
    /// Double-copy bypass window in seconds. Set to 0 to disable the bypass.
    pub double_copy_window: u64,
    /// Default notification "Disable" action timeout in seconds. Set to 0 to hide the action.
    pub disable_for: u64,
    /// Controls non-actionable desktop lifecycle notifications.
    pub notifications: NotificationConfig,
    /// Source applications to filter globally. Values match bundle id or app name.
    pub apps: Vec<String>,
    /// How to interpret apps globally: blacklist skips listed apps; whitelist only allows listed apps.
    pub app_mode: Option<AppMode>,
    /// URL import refresh interval in seconds. Set to 0 to never download URL imports.
    pub import_refresh_interval: u64,
    /// Explicit editor command and argument templates used by Edit rule.
    pub editor: Option<EditorConfig>,
    /// Host-owned authorization for native shell rule providers.
    pub shell: ShellConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct NotificationConfig {
    /// Notify after the desktop app starts successfully.
    pub startup: bool,
    /// Notify after a changed configuration is applied successfully.
    pub reload_success: bool,
    /// Notify after clipboard content is transformed successfully.
    pub transform: bool,
    /// Notify when a double copy bypasses configured rules.
    pub double_copy_ignored: bool,
    /// Notify when one or more plugins require attention.
    pub plugin_attention: bool,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            startup: true,
            reload_success: true,
            transform: true,
            double_copy_ignored: true,
            plugin_attention: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ShellConfig {
    /// Enables trusted shell and item-shell rules for this native host.
    pub enabled: bool,
    /// Permits shell rules declared by local filesystem imports.
    pub local_imports: bool,
    /// Permits explicitly pinned shell rules declared by URL imports.
    pub remote_imports: bool,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            local_imports: true,
            remote_imports: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EditorConfig {
    /// Editor executable or launcher path. Arguments belong in `args`.
    #[schemars(length(min = 1))]
    pub command: String,
    /// Argument templates. Supports {file}, {line}, and {column}.
    #[serde(default)]
    pub args: Vec<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            recent_items_count: 5,
            max_item_bytes: 100 * 1024 * 1024,
            max_history_bytes: 512 * 1024 * 1024,
            persist_last_clipboard: false,
            double_copy_window: 10,
            disable_for: 600,
            notifications: NotificationConfig::default(),
            apps: Vec::new(),
            app_mode: None,
            import_refresh_interval: 600,
            editor: None,
            shell: ShellConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn app_matcher(&self) -> Result<AppMatcher> {
        AppMatcher::compile(self.apps.clone(), self.app_mode)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ConfigDocument {
    pub config: AppConfig,
    pub rules: Vec<RawRule>,
    /// Per-plugin permissions and settings, keyed by plugin id. Imported
    /// documents intentionally contribute rules only.
    pub plugins: BTreeMap<String, PluginConfig>,
    /// Rule group descriptors, keyed by group ID.
    #[serde(default)]
    pub groups: BTreeMap<String, GroupDescriptor>,
    /// Imports of group descriptors from other configuration documents.
    #[serde(default, rename = "group_imports")]
    pub group_imports: Vec<GroupImport>,
}

/// Per-plugin configuration under the top-level `plugins` mapping.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PluginConfig {
    /// Host-enforced capability grants for this plugin.
    pub permissions: PluginPermissions,
    /// Opaque plugin-owned settings. The host does not define keys inside it.
    pub settings: serde_json::Value,
}

/// Host-owned capability grants. Effective capabilities are the intersection
/// of manifest-requested capabilities and these grants.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PluginPermissions {
    /// Hostname patterns passed to the runtime's network policy.
    pub http: Vec<String>,
    /// Expands `$VAR`-style references in plugin settings before initialization.
    pub env_expansion: bool,
}

/// Visibility and activation policy for a rule group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum GroupStatus {
    /// The group is functional and shown in the desktop tray as a switch.
    #[default]
    Visible,
    /// The group is functional but not shown in the tray.
    Hidden,
    /// The group label is removed from effective membership and ignored.
    Ignore,
}

/// Presentation metadata for a rule group. The map key is the group ID.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct GroupDescriptor {
    /// Optional short display label. Falls back to the group ID.
    pub name: Option<String>,
    /// Optional longer description for diagnostics and tray tooltips.
    pub description: Option<String>,
    /// Tray visibility and activation policy.
    pub status: GroupStatus,
}

/// Import of group descriptors from another configuration document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupImport {
    /// Path, file: URL, http: URL, or https: URL to import.
    pub source: String,
    /// Default status for descriptors from this source. Defaults to hidden.
    #[serde(default)]
    pub status: Option<GroupStatus>,
}

/// Controls which memberships authored inside an imported rule subtree are
/// discarded before groups from the importing edge are applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum IgnoreImportedGroups {
    /// `true` strips every imported group. `false` is accepted as an explicit
    /// no-op so the generated schema and runtime parser stay identical.
    All(bool),
    /// Strips only the listed group IDs.
    List(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Yaml,
    Toml,
}

impl ConfigFormat {
    pub fn from_path(path: &Path) -> Result<Self> {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("yaml" | "yml") => Ok(Self::Yaml),
            Some("toml") => Ok(Self::Toml),
            _ => bail!("unsupported config extension for {}", path.display()),
        }
    }
}

/// Parses one self-contained document. Import resolution and host I/O remain
/// the responsibility of the caller.
pub fn parse_document(text: &str, format: ConfigFormat) -> Result<ConfigDocument> {
    match format {
        ConfigFormat::Yaml => serde_yaml::from_str(text).context("parse YAML config"),
        ConfigFormat::Toml => toml::from_str(text).context("parse TOML config"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_yaml_document_parses_without_host_services() {
        let document = parse_document(
            "config:\n  max_item_bytes: 42\nrules: []\n",
            ConfigFormat::Yaml,
        )
        .unwrap();
        assert_eq!(document.config.max_item_bytes, 42);
        assert!(document.config.notifications.startup);
        assert!(document.config.notifications.reload_success);
        assert!(document.config.notifications.transform);
        assert!(document.config.notifications.double_copy_ignored);
        assert!(document.config.notifications.plugin_attention);
    }

    #[test]
    fn notification_preferences_can_be_disabled_independently() {
        let startup_disabled = parse_document(
            "config:\n  notifications:\n    startup: false\nrules: []\n",
            ConfigFormat::Yaml,
        )
        .unwrap();
        assert!(!startup_disabled.config.notifications.startup);
        assert!(startup_disabled.config.notifications.reload_success);
        assert!(startup_disabled.config.notifications.transform);
        assert!(startup_disabled.config.notifications.double_copy_ignored);
        assert!(startup_disabled.config.notifications.plugin_attention);

        let reload_disabled = parse_document(
            "config:\n  notifications:\n    reload_success: false\nrules: []\n",
            ConfigFormat::Yaml,
        )
        .unwrap();
        assert!(reload_disabled.config.notifications.startup);
        assert!(!reload_disabled.config.notifications.reload_success);

        let transform_disabled = parse_document(
            "config:\n  notifications:\n    transform: false\nrules: []\n",
            ConfigFormat::Yaml,
        )
        .unwrap();
        assert!(transform_disabled.config.notifications.startup);
        assert!(transform_disabled.config.notifications.reload_success);
        assert!(!transform_disabled.config.notifications.transform);
        assert!(transform_disabled.config.notifications.double_copy_ignored);
        assert!(transform_disabled.config.notifications.plugin_attention);
    }

    #[test]
    fn plugin_config_parses_settings_and_permissions() {
        let config: PluginConfig = serde_yaml::from_str(
            r#"
permissions:
  http:
    - gitlab.example.com
  env_expansion: true
settings:
  instances:
    - id: work
      token: ${GITLAB_TOKEN}
"#,
        )
        .unwrap();
        assert!(config.permissions.env_expansion);
        assert_eq!(config.permissions.http, ["gitlab.example.com"]);
        assert!(config.settings.get("instances").is_some());
    }

    #[test]
    fn unknown_permission_keys_are_rejected() {
        let error = serde_yaml::from_str::<PluginConfig>("permissions:\n  sockets: true\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains("sockets"), "{error}");
    }
}
