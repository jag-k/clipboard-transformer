//! WASM plugin system: discovery, capability grants, initialization, and
//! integration with the rule engine.
//!
//! The product protocol lives in the runtime-neutral `ct-plugin-api` crate and
//! stays independent of the Extism adapter in [`runtime`].

mod config;
mod expansion;
mod manifest;
mod provider;
mod runtime;

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub use config::{PluginConfig, PluginPermissions};
pub use expansion::{expand_settings, expand_str};
pub use manifest::{extract_manifest, MAX_MANIFEST_BYTES, MAX_MODULE_BYTES};
pub use runtime::{PluginLimits, PluginRuntime, REQUIRED_EXPORTS};

use ct_core::{ExternalRuleProvider, RawRule};
use ct_plugin_api::{
    namespaced_rule_type, AttentionLevel, CapabilityKind, GrantedCapabilities, InitializeRequest,
    IssueSeverity, PluginHealth, PluginIssue, PluginManifest, PLUGIN_API_VERSION,
};

/// Discovery result for one `.wasm` file, before initialization.
pub struct CatalogEntry {
    pub path: PathBuf,
    /// Module bytes; empty when reading failed.
    module: Vec<u8>,
    /// Embedded manifest, or the discovery failure.
    pub manifest: Result<PluginManifest, String>,
}

/// Discovered plugin modules and their static manifests. Discovery never
/// executes plugin code and never fails as a whole: unreadable or invalid
/// files become failed entries so they stay visible to CLI and tray.
pub struct PluginCatalog {
    pub entries: Vec<CatalogEntry>,
}

impl PluginCatalog {
    /// Discovers `*.wasm` modules in `dir`, sorted by file name. A missing
    /// directory yields an empty catalog.
    pub fn discover(dir: &Path) -> Self {
        let entries = Self::module_paths(dir)
            .into_iter()
            .map(Self::read_entry)
            .collect();
        Self { entries }
    }

    /// Discovers module paths and embedded manifests like [`Self::discover`],
    /// but drops each module's bytes as soon as its manifest is extracted, so
    /// peak memory stays one module instead of the whole directory. Use for
    /// checks that never instantiate plugins.
    pub fn discover_manifests(dir: &Path) -> Vec<(PathBuf, Result<PluginManifest, String>)> {
        Self::module_paths(dir)
            .into_iter()
            .map(|path| {
                let entry = Self::read_entry(path);
                (entry.path, entry.manifest)
            })
            .collect()
    }

    fn module_paths(dir: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Ok(reader) = fs::read_dir(dir) {
            for entry in reader.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) == Some("wasm") && path.is_file() {
                    paths.push(path);
                }
            }
        }
        paths.sort();
        paths
    }

    fn read_entry(path: PathBuf) -> CatalogEntry {
        let module = match fs::metadata(&path) {
            Ok(metadata) if metadata.len() > MAX_MODULE_BYTES => Err(format!(
                "module is {} bytes; the limit is {MAX_MODULE_BYTES}",
                metadata.len()
            )),
            _ => fs::read(&path).map_err(|error| format!("read module: {error}")),
        };
        match module {
            Ok(module) => {
                let manifest = extract_manifest(&module).map_err(|error| format!("{error:#}"));
                CatalogEntry {
                    path,
                    module,
                    manifest,
                }
            }
            Err(error) => CatalogEntry {
                path,
                module: Vec::new(),
                manifest: Err(error),
            },
        }
    }

    /// Content fingerprints per module path, used by hot reload to detect
    /// added, removed, or changed plugin files.
    pub fn module_fingerprints(&self) -> BTreeMap<PathBuf, u64> {
        self.entries
            .iter()
            .map(|entry| {
                let mut hasher = DefaultHasher::new();
                entry.module.hash(&mut hasher);
                (entry.path.clone(), hasher.finish())
            })
            .collect()
    }

    /// Namespaced rule type ids contributed by every valid manifest. Config
    /// loading uses this to keep plugin-typed rules for later validation.
    pub fn known_rule_types(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .filter_map(|entry| entry.manifest.as_ref().ok())
            .flat_map(|manifest| manifest.rule_type_ids())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Initializes every discovered plugin against the user configuration
    /// using the process environment for granted expansion.
    pub fn initialize(
        self,
        configs: &BTreeMap<String, PluginConfig>,
        limits: &PluginLimits,
    ) -> PluginSet {
        self.initialize_with_env(configs, limits, &crate::platform::environment::var)
    }

    /// Initializes only plugins referenced by configured rules. Other valid
    /// modules remain visible as `available` without creating an Extism
    /// instance. Nested rulesets are traversed recursively.
    pub fn initialize_for_rules(
        self,
        configs: &BTreeMap<String, PluginConfig>,
        limits: &PluginLimits,
        rules: &[RawRule],
    ) -> PluginSet {
        let mut referenced = BTreeSet::new();
        collect_rule_types(rules, &mut referenced);
        self.initialize_internal(
            configs,
            limits,
            &crate::platform::environment::var,
            Some(&referenced),
        )
    }

    /// Initialization with an explicit environment lookup, for tests.
    pub fn initialize_with_env(
        self,
        configs: &BTreeMap<String, PluginConfig>,
        limits: &PluginLimits,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> PluginSet {
        self.initialize_internal(configs, limits, env, None)
    }

    fn initialize_internal(
        self,
        configs: &BTreeMap<String, PluginConfig>,
        limits: &PluginLimits,
        env: &dyn Fn(&str) -> Option<String>,
        referenced_rule_types: Option<&BTreeSet<String>>,
    ) -> PluginSet {
        let mut id_counts: BTreeMap<&str, usize> = BTreeMap::new();
        for entry in &self.entries {
            if let Ok(manifest) = &entry.manifest {
                *id_counts.entry(manifest.id.as_str()).or_default() += 1;
            }
        }
        let duplicate_ids: BTreeSet<String> = id_counts
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(id, _)| id.to_string())
            .collect();
        let mut statuses = Vec::new();
        let mut providers: Vec<Arc<dyn ExternalRuleProvider>> = Vec::new();
        for entry in self.entries {
            let status = initialize_entry(
                entry,
                configs,
                limits,
                env,
                referenced_rule_types,
                &duplicate_ids,
                &mut providers,
            );
            statuses.push(status);
        }
        PluginSet {
            statuses,
            providers,
        }
    }
}

/// Host-observed plugin state. `Available` means discovery succeeded but the
/// current rules do not require an instance. `Operational`, `Degraded`, and
/// `Blocked` come from plugin initialization; `Failed` is host-owned (invalid
/// module, incompatible API, trap, or timeout).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginState {
    Available,
    Operational,
    Degraded,
    Blocked,
    Failed,
}

impl PluginState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Operational => "operational",
            Self::Degraded => "degraded",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
        }
    }
}

impl From<PluginHealth> for PluginState {
    fn from(health: PluginHealth) -> Self {
        match health {
            PluginHealth::Operational => Self::Operational,
            PluginHealth::Degraded => Self::Degraded,
            PluginHealth::Blocked => Self::Blocked,
        }
    }
}

/// Everything the CLI, tray, and notifications need to describe one plugin.
#[derive(Debug)]
pub struct PluginStatus {
    pub path: PathBuf,
    /// Manifest id, or the file stem when the manifest could not be read.
    pub id: String,
    pub manifest: Option<PluginManifest>,
    pub state: PluginState,
    pub issues: Vec<PluginIssue>,
    /// Namespaced rule type ids currently usable in the config.
    pub available_rules: Vec<String>,
    /// Effective Extism patterns retained by the host for diagnostics and
    /// runtime policy. This field is never serialized into plugin requests.
    pub granted_http_hosts: Vec<String>,
    pub granted: GrantedCapabilities,
}

/// Host-only capability resolution. `plugin` is safe to serialize into the
/// protocol; HTTP patterns stay here and are exposed only through the
/// `http_host_allowed` boolean host function.
#[derive(Debug, Clone, Default, PartialEq)]
struct ResolvedGrants {
    plugin: GrantedCapabilities,
    http_hosts: Vec<String>,
}

impl PluginStatus {
    pub fn requires_attention(&self) -> bool {
        self.state == PluginState::Failed
            || self
                .issues
                .iter()
                .any(|issue| issue.attention == AttentionLevel::ActionRequired)
    }
}

/// Initialized plugins plus the rule providers they contribute.
pub struct PluginSet {
    statuses: Vec<PluginStatus>,
    providers: Vec<Arc<dyn ExternalRuleProvider>>,
}

impl PluginSet {
    pub fn empty() -> Self {
        Self {
            statuses: Vec::new(),
            providers: Vec::new(),
        }
    }

    pub fn statuses(&self) -> &[PluginStatus] {
        &self.statuses
    }

    /// Consumes the set, keeping only the display statuses. The providers
    /// (and the plugin instances they share) live on inside the compiled
    /// rule engine.
    pub fn into_statuses(self) -> Vec<PluginStatus> {
        self.statuses
    }

    pub fn providers(&self) -> &[Arc<dyn ExternalRuleProvider>] {
        &self.providers
    }

    pub fn is_empty(&self) -> bool {
        self.statuses.is_empty()
    }

    pub fn attention_count(&self) -> usize {
        self.statuses
            .iter()
            .filter(|status| status.requires_attention())
            .count()
    }

    /// Stable-within-process fingerprint of every plugin's issues, used to
    /// deduplicate "requires attention" notifications across hot reloads.
    pub fn issue_fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        for status in &self.statuses {
            status.id.hash(&mut hasher);
            status.state.hash(&mut hasher);
            for issue in &status.issues {
                issue.code.hash(&mut hasher);
                issue.summary.hash(&mut hasher);
            }
        }
        hasher.finish()
    }
}

impl std::fmt::Debug for PluginSet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginSet")
            .field("statuses", &self.statuses)
            .field("provider_count", &self.providers.len())
            .finish()
    }
}

fn initialize_entry(
    entry: CatalogEntry,
    configs: &BTreeMap<String, PluginConfig>,
    limits: &PluginLimits,
    env: &dyn Fn(&str) -> Option<String>,
    referenced_rule_types: Option<&BTreeSet<String>>,
    duplicate_ids: &BTreeSet<String>,
    providers: &mut Vec<Arc<dyn ExternalRuleProvider>>,
) -> PluginStatus {
    let fallback_id = entry
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown-plugin")
        .to_string();
    let manifest = match entry.manifest {
        Ok(manifest) => manifest,
        Err(error) => {
            return failed_status(
                entry.path,
                fallback_id,
                None,
                "invalid-module",
                format!("plugin module could not be loaded: {error}"),
            );
        }
    };

    // Ambiguous identity disables every claimant: with two modules claiming
    // one id there is no safe way to pick which one the user meant.
    if duplicate_ids.contains(&manifest.id) {
        let id = manifest.id.clone();
        return failed_status(
            entry.path,
            id.clone(),
            Some(manifest),
            "duplicate-plugin-id",
            format!("plugin id {id:?} is provided by multiple modules; all of them are disabled"),
        );
    }

    if referenced_rule_types.is_some_and(|referenced| {
        !manifest
            .rule_type_ids()
            .any(|rule_type| referenced.contains(&rule_type))
    }) {
        let available_rules = manifest.rule_type_ids().collect();
        return PluginStatus {
            path: entry.path,
            id: manifest.id.clone(),
            manifest: Some(manifest),
            state: PluginState::Available,
            issues: Vec::new(),
            available_rules,
            granted_http_hosts: Vec::new(),
            granted: GrantedCapabilities::default(),
        };
    }

    let plugin_config = configs.get(&manifest.id).cloned().unwrap_or_default();
    let mut issues = Vec::new();
    let granted = resolve_grants(&manifest, &plugin_config.permissions, &mut issues);

    let settings = if granted.plugin.env_expansion {
        match expand_settings(&plugin_config.settings, env) {
            Ok(settings) => settings,
            Err(error) => {
                // A failed required expansion blocks the plugin without
                // calling it, per the draft.
                issues.push(PluginIssue::host(
                    "env-expansion-failed",
                    IssueSeverity::Error,
                    format!("environment expansion failed: {error:#}"),
                ));
                return PluginStatus {
                    path: entry.path,
                    id: manifest.id.clone(),
                    manifest: Some(manifest),
                    state: PluginState::Blocked,
                    issues,
                    available_rules: Vec::new(),
                    granted_http_hosts: granted.http_hosts,
                    granted: granted.plugin,
                };
            }
        }
    } else {
        plugin_config.settings.clone()
    };

    let mut runtime = match PluginRuntime::load(&entry.module, &granted.http_hosts, limits) {
        Ok(runtime) => runtime,
        Err(error) => {
            return failed_status(
                entry.path,
                manifest.id.clone(),
                Some(manifest),
                "instantiate-failed",
                format!("plugin could not be instantiated: {error:#}"),
            );
        }
    };

    let request = InitializeRequest {
        api_version: PLUGIN_API_VERSION,
        settings,
        granted_capabilities: granted.plugin.clone(),
    };
    let response = match runtime.initialize(&request) {
        Ok(response) => response,
        Err(error) => {
            return failed_status(
                entry.path,
                manifest.id.clone(),
                Some(manifest),
                "initialize-failed",
                format!("plugin initialization failed: {error:#}"),
            );
        }
    };

    issues.extend(response.issues);
    let declared: BTreeSet<&str> = manifest
        .rules
        .iter()
        .map(|rule| rule.rule_type.as_str())
        .collect();
    let available_local: Vec<String> = match response.available_rules {
        None => declared.iter().map(|name| name.to_string()).collect(),
        Some(reported) => {
            let mut available = Vec::new();
            for name in reported {
                if declared.contains(name.as_str()) {
                    if !available.contains(&name) {
                        available.push(name);
                    }
                } else {
                    issues.push(PluginIssue {
                        code: "unknown-rule-type".to_string(),
                        severity: IssueSeverity::Warning,
                        summary: format!("initialization reported undeclared rule type {name:?}"),
                        details: None,
                        setting_path: None,
                        rule_types: Vec::new(),
                        attention: AttentionLevel::Informational,
                    });
                }
            }
            available
        }
    };

    let state = PluginState::from(response.status);
    let mut available_rules = Vec::new();
    if state != PluginState::Blocked {
        let shared = Arc::new(Mutex::new(runtime));
        for descriptor in &manifest.rules {
            if !available_local.contains(&descriptor.rule_type) {
                continue;
            }
            let kind = namespaced_rule_type(&manifest.id, &descriptor.rule_type);
            available_rules.push(kind.clone());
            providers.push(Arc::new(provider::PluginRuleProvider::new(
                kind,
                descriptor.rule_type.clone(),
                descriptor.formats.clone(),
                Arc::clone(&shared),
            )));
        }
    }

    PluginStatus {
        path: entry.path,
        id: manifest.id.clone(),
        manifest: Some(manifest),
        state,
        issues,
        available_rules,
        granted_http_hosts: granted.http_hosts,
        granted: granted.plugin,
    }
}

fn collect_rule_types(rules: &[RawRule], output: &mut BTreeSet<String>) {
    for rule in rules {
        if let Some(kind) = rule.kind.as_deref() {
            output.insert(kind.to_string());
        }
        collect_rule_types(&rule.rules, output);
    }
}

fn failed_status(
    path: PathBuf,
    id: String,
    manifest: Option<PluginManifest>,
    code: &str,
    summary: String,
) -> PluginStatus {
    PluginStatus {
        path,
        id,
        manifest,
        state: PluginState::Failed,
        issues: vec![PluginIssue::host(code, IssueSeverity::Error, summary)],
        available_rules: Vec::new(),
        granted_http_hosts: Vec::new(),
        granted: GrantedCapabilities::default(),
    }
}

/// Computes `requested ∩ configured` capabilities and reports undeclared or
/// invalid grants as issues without granting access.
fn resolve_grants(
    manifest: &PluginManifest,
    permissions: &PluginPermissions,
    issues: &mut Vec<PluginIssue>,
) -> ResolvedGrants {
    let mut granted = ResolvedGrants::default();

    if !permissions.http.is_empty() {
        if !manifest.requests_capability(CapabilityKind::Http) {
            issues.push(undeclared_grant_issue("http"));
        } else {
            granted.http_hosts = permissions.http.clone();
        }
    }

    if permissions.env_expansion {
        if !manifest.requests_capability(CapabilityKind::EnvExpansion) {
            issues.push(undeclared_grant_issue("env_expansion"));
        } else {
            granted.plugin.env_expansion = true;
        }
    }

    granted
}

fn undeclared_grant_issue(grant: &str) -> PluginIssue {
    PluginIssue {
        code: "undeclared-grant".to_string(),
        severity: IssueSeverity::Warning,
        summary: format!(
            "config grants {grant:?} but the plugin manifest does not request it; not granted"
        ),
        details: None,
        setting_path: None,
        rule_types: Vec::new(),
        attention: AttentionLevel::Informational,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, capabilities: serde_json::Value) -> PluginManifest {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": "Test",
            "version": "0.1.0",
            "api_version": 1,
            "rules": [{"type": "demo"}],
            "capabilities": capabilities,
        }))
        .unwrap()
    }

    #[test]
    fn grants_are_the_intersection_of_requested_and_configured() {
        let manifest = manifest(
            "dev.example.test",
            serde_json::json!([{"kind": "http"}, {"kind": "env-expansion"}]),
        );
        let permissions: PluginPermissions =
            serde_yaml::from_str("http: [\"gitlab.example.com\"]\nenv_expansion: true\n").unwrap();
        let mut issues = Vec::new();
        let granted = resolve_grants(&manifest, &permissions, &mut issues);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(granted.http_hosts, ["gitlab.example.com"]);
        assert!(granted.plugin.env_expansion);
    }

    #[test]
    fn undeclared_grants_warn_and_do_not_grant() {
        let manifest = manifest("dev.example.test", serde_json::json!([]));
        let permissions: PluginPermissions =
            serde_yaml::from_str("http: [\"gitlab.example.com\"]\nenv_expansion: true\n").unwrap();
        let mut issues = Vec::new();
        let granted = resolve_grants(&manifest, &permissions, &mut issues);
        assert_eq!(granted, ResolvedGrants::default());
        assert_eq!(issues.len(), 2);
        assert!(issues.iter().all(|issue| issue.code == "undeclared-grant"));
    }

    #[test]
    fn extism_host_globs_are_passed_through_verbatim() {
        let manifest = manifest("dev.example.test", serde_json::json!([{"kind": "http"}]));
        let permissions: PluginPermissions =
            serde_yaml::from_str("http: [\"*.example.com\", \"*\"]\n").unwrap();
        let mut issues = Vec::new();
        let granted = resolve_grants(&manifest, &permissions, &mut issues);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(granted.http_hosts, ["*.example.com", "*"]);
    }

    #[test]
    fn discovery_of_missing_directory_is_empty() {
        let catalog = PluginCatalog::discover(Path::new("/nonexistent/plugins"));
        assert!(catalog.is_empty());
        assert!(catalog.known_rule_types().is_empty());
    }

    #[test]
    fn discovery_reports_invalid_modules_without_failing() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("broken.wasm"), b"not wasm").unwrap();
        fs::write(dir.path().join("ignored.txt"), b"not a module").unwrap();
        let catalog = PluginCatalog::discover(dir.path());
        assert_eq!(catalog.entries.len(), 1);
        assert!(catalog.entries[0].manifest.is_err());

        let set = catalog.initialize(&BTreeMap::new(), &PluginLimits::default());
        assert_eq!(set.statuses().len(), 1);
        assert_eq!(set.statuses()[0].state, PluginState::Failed);
        assert_eq!(set.statuses()[0].id, "broken");
        assert!(set.statuses()[0].requires_attention());
        assert!(set.providers().is_empty());
    }

    #[test]
    fn duplicate_plugin_ids_disable_every_claimant() {
        let entry = |name: &str| CatalogEntry {
            path: PathBuf::from(name),
            module: Vec::new(),
            manifest: Ok(manifest("dev.example.dup", serde_json::json!([]))),
        };
        let catalog = PluginCatalog {
            entries: vec![
                entry("a.wasm"),
                entry("b.wasm"),
                CatalogEntry {
                    path: PathBuf::from("c.wasm"),
                    module: Vec::new(),
                    manifest: Ok(manifest("dev.example.unique", serde_json::json!([]))),
                },
            ],
        };

        let set =
            catalog.initialize_with_env(&BTreeMap::new(), &PluginLimits::default(), &|_| None);

        assert_eq!(set.statuses().len(), 3);
        for status in &set.statuses()[..2] {
            assert_eq!(status.state, PluginState::Failed, "{status:?}");
            assert!(
                status
                    .issues
                    .iter()
                    .any(|issue| issue.code == "duplicate-plugin-id"),
                "{status:?}"
            );
        }
        assert_ne!(
            set.statuses()[2]
                .issues
                .first()
                .map(|issue| issue.code.as_str()),
            Some("duplicate-plugin-id"),
            "unique id must not be affected"
        );
    }

    #[test]
    fn referenced_rule_types_include_nested_plugin_rules() {
        let rules = vec![RawRule {
            kind: Some("ruleset".into()),
            id: "outer".into(),
            rules: vec![RawRule {
                kind: Some("dev.example.plugin/demo".into()),
                id: "plugin-rule".into(),
                ..RawRule::default()
            }],
            ..RawRule::default()
        }];
        let mut referenced = BTreeSet::new();

        collect_rule_types(&rules, &mut referenced);

        assert!(referenced.contains("ruleset"));
        assert!(referenced.contains("dev.example.plugin/demo"));
    }
}
