mod defaults;
mod paths;
mod schema;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value as YamlValue};
use sha2::{Digest, Sha256};
use url::Url;

pub use defaults::{
    ensure_default_config, sync_config_schema_contents_next_to, sync_config_schema_next_to,
    write_config_schema, write_config_schema_next_to, CONFIG_SCHEMA_FILE_NAME, DEFAULT_CONFIG_YAML,
};
pub use paths::{short_path_for_display, ConfigPaths};
pub use schema::{
    json_schema, json_schema_pretty, json_schema_pretty_with_plugins, plugin_schema_contributions,
    PluginRuleSchemaContribution,
};

use ct_core::{is_registered_rule_type, AppMode, RawRule, RuleEngine};

pub use ct_config::{
    AppConfig, ConfigDocument, ConfigFormat, EditorConfig, NotificationConfig, ShellConfig,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleSource {
    pub path: PathBuf,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedConfig {
    pub document: ConfigDocument,
    pub sources: BTreeSet<PathBuf>,
    pub remote_imports: BTreeMap<String, PathBuf>,
    pub rule_sources: BTreeMap<String, RuleSource>,
    pub warnings: Vec<ConfigWarning>,
}

type ShellSourcePolicy<'a> = (
    &'a Path,
    &'a BTreeSet<PathBuf>,
    &'a BTreeSet<(PathBuf, String)>,
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigLoadOptions {
    pub state_dir: Option<PathBuf>,
    pub refresh_url_imports: bool,
    /// Rule `type` values provided by discovered plugins. Rules with these
    /// types are kept for later plugin-side validation instead of being
    /// dropped as unknown.
    pub known_rule_types: BTreeSet<String>,
}

impl Default for ConfigLoadOptions {
    fn default() -> Self {
        Self {
            state_dir: None,
            refresh_url_imports: true,
            known_rule_types: BTreeSet::new(),
        }
    }
}

fn imported_config_format(path: &Path) -> Result<ConfigFormat> {
    if path.extension().is_some() {
        return ConfigFormat::from_path(path);
    }

    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if serde_yaml::from_str::<YamlValue>(&text).is_ok_and(|value| imported_yaml_shape(&value)) {
        return Ok(ConfigFormat::Yaml);
    }
    if toml::from_str::<toml::Value>(&text)
        .is_ok_and(|value| value.get("rules").is_some_and(|rules| rules.is_array()))
    {
        return Ok(ConfigFormat::Toml);
    }
    bail!(
        "extensionless import {} is neither a YAML nor TOML rules document",
        path.display()
    )
}

fn imported_yaml_shape(value: &YamlValue) -> bool {
    value.is_sequence()
        || value.as_mapping().is_some_and(|mapping| {
            mapping
                .get(YamlValue::String("rules".to_string()))
                .is_some_and(YamlValue::is_sequence)
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ConfigWarning {
    ImportCycle {
        chain: Vec<PathBuf>,
    },
    DuplicateRuleId {
        id: String,
    },
    EmptyGlobalAppWhitelist,
    EmptyRuleAppWhitelist {
        id: String,
    },
    IgnoredRuleType {
        kind: String,
    },
    InvalidRule {
        id: Option<String>,
        kind: String,
        reason: String,
    },
}

impl fmt::Display for ConfigWarning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ImportCycle { chain } => {
                let chain = chain
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" -> ");
                write!(formatter, "import cycle ignored: {chain}")
            }
            Self::DuplicateRuleId { id } => write!(formatter, "duplicate rule id {id:?}"),
            Self::EmptyGlobalAppWhitelist => {
                formatter.write_str("global app whitelist is empty; no applications will match")
            }
            Self::EmptyRuleAppWhitelist { id } => write!(
                formatter,
                "rule {id:?} app whitelist is empty; the rule will never match"
            ),
            Self::IgnoredRuleType { kind } => {
                write!(
                    formatter,
                    "rule type {kind:?} ignored: no registered handler"
                )
            }
            Self::InvalidRule { id, kind, reason } => {
                if let Some(id) = id {
                    write!(formatter, "rule {id:?} ({kind}) ignored: {reason}")
                } else {
                    write!(formatter, "{kind} rule without an id ignored: {reason}")
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    pub warnings: Vec<ConfigWarning>,
}

impl ValidationReport {
    pub fn is_clean(&self) -> bool {
        self.warnings.is_empty()
    }
}

pub fn load_config(path: impl AsRef<Path>) -> Result<ConfigDocument> {
    Ok(load_config_with_sources(path)?.document)
}

pub fn load_config_with_sources(path: impl AsRef<Path>) -> Result<LoadedConfig> {
    load_config_with_options(path, ConfigLoadOptions::default())
}

pub fn load_config_with_options(
    path: impl AsRef<Path>,
    options: ConfigLoadOptions,
) -> Result<LoadedConfig> {
    let path = path.as_ref();
    match ConfigFormat::from_path(path)? {
        ConfigFormat::Yaml => load_yaml(path, &options),
        ConfigFormat::Toml => load_toml(path, &options),
    }
}

/// Parses a self-contained YAML or TOML config supplied by an embedding host.
///
/// Inline configs deliberately do not expand imports: they have no stable
/// filesystem base directory and must not cause implicit filesystem or network
/// access.
pub fn load_inline_config(text: &str, known_rule_types: BTreeSet<String>) -> Result<LoadedConfig> {
    let document = serde_yaml::from_str::<ConfigDocument>(text)
        .or_else(|yaml_error| {
            toml::from_str::<ConfigDocument>(text).map_err(|toml_error| {
                anyhow!(
                    "inline config is neither valid YAML ({yaml_error}) nor TOML ({toml_error})"
                )
            })
        })
        .context("parse inline config")?;
    Ok(finish_loaded_config(
        document,
        BTreeSet::new(),
        BTreeMap::new(),
        Vec::new(),
        &known_rule_types,
        None,
    ))
}

pub fn collect_config_sources_best_effort(path: impl AsRef<Path>) -> BTreeSet<PathBuf> {
    collect_config_sources_best_effort_with_options(
        path,
        ConfigLoadOptions {
            refresh_url_imports: false,
            ..ConfigLoadOptions::default()
        },
    )
}

pub fn collect_config_sources_best_effort_with_options(
    path: impl AsRef<Path>,
    options: ConfigLoadOptions,
) -> BTreeSet<PathBuf> {
    let path = path.as_ref();
    match ConfigFormat::from_path(path) {
        Ok(ConfigFormat::Yaml) => {
            let mut stack = VecDeque::new();
            let mut sources = BTreeSet::new();
            let mut context = LoadContext::new(options);
            collect_yaml_sources(path, &mut stack, &mut sources, &mut context);
            sources
        }
        Ok(ConfigFormat::Toml) => {
            let mut stack = VecDeque::new();
            let mut sources = BTreeSet::new();
            let mut context = LoadContext::new(options);
            collect_toml_sources(path, &mut stack, &mut sources, &mut context);
            sources
        }
        Err(_) => [path.to_path_buf()].into_iter().collect(),
    }
}

pub fn validate_config(
    path: impl AsRef<Path>,
    options: ConfigLoadOptions,
) -> Result<ValidationReport> {
    let known_rule_types = effective_known_rule_types(&options.known_rule_types);
    let loaded = load_config_with_options(path, options)?;
    validate_loaded_config(loaded, &known_rule_types)
}

/// Validates a config that has already been loaded from a file or inline text.
pub fn validate_loaded_config(
    loaded: LoadedConfig,
    known_rule_types: &BTreeSet<String>,
) -> Result<ValidationReport> {
    let known_rule_types = effective_known_rule_types(known_rule_types);
    loaded.document.config.app_matcher()?;
    // Plugin-typed rules validate against the initialized plugin, not here;
    // compile only the built-in parts.
    let builtin_rules = loaded
        .document
        .rules
        .iter()
        .filter(|rule| is_registered_rule_type(rule.kind.as_deref().unwrap_or("regexp")))
        .map(|rule| strip_plugin_rules(rule, &known_rule_types))
        .filter(|rule| rule.kind.as_deref() != Some("ruleset") || !rule.rules.is_empty())
        .collect();
    RuleEngine::compile(builtin_rules)?;
    Ok(ValidationReport {
        warnings: loaded.warnings,
    })
}

fn finish_loaded_config(
    mut document: ConfigDocument,
    sources: BTreeSet<PathBuf>,
    remote_imports: BTreeMap<String, PathBuf>,
    mut warnings: Vec<ConfigWarning>,
    known_rule_types: &BTreeSet<String>,
    shell_sources: Option<ShellSourcePolicy<'_>>,
) -> LoadedConfig {
    let known_rule_types = effective_known_rule_types(known_rule_types);
    filter_invalid_rules(&mut document.rules, &mut warnings, &known_rule_types);

    if document.config.app_mode == Some(AppMode::Whitelist) && document.config.apps.is_empty() {
        push_warning(&mut warnings, ConfigWarning::EmptyGlobalAppWhitelist);
    }

    let mut rule_ids = BTreeSet::new();
    collect_rule_warnings(&document.rules, &mut rule_ids, &mut warnings);
    let rule_sources = find_rule_sources(&sources, &rule_ids);
    if let Some((root, remote_sources, authorized_remote_shell_rules)) = shell_sources {
        filter_unauthorized_shell_rules(
            &mut document.rules,
            &document.config.shell,
            root,
            remote_sources,
            authorized_remote_shell_rules,
            &rule_sources,
            &mut warnings,
        );
    }
    LoadedConfig {
        document,
        sources,
        remote_imports,
        rule_sources,
        warnings,
    }
}

fn filter_unauthorized_shell_rules(
    rules: &mut Vec<RawRule>,
    policy: &ShellConfig,
    root: &Path,
    remote_sources: &BTreeSet<PathBuf>,
    authorized_remote_shell_rules: &BTreeSet<(PathBuf, String)>,
    rule_sources: &BTreeMap<String, RuleSource>,
    warnings: &mut Vec<ConfigWarning>,
) {
    rules.retain_mut(|rule| {
        filter_unauthorized_shell_rules(
            &mut rule.rules,
            policy,
            root,
            remote_sources,
            authorized_remote_shell_rules,
            rule_sources,
            warnings,
        );
        let kind = rule.kind.as_deref().unwrap_or("regexp");
        if !matches!(kind, "shell" | "item-shell") {
            return true;
        }
        let source = rule_sources
            .get(&rule.id)
            .map(|source| source.path.as_path());
        let allowed = policy.enabled
            && source.is_some_and(|source| {
                if source == root {
                    true
                } else if remote_sources.contains(source) {
                    authorized_remote_shell_rules
                        .contains(&(source.to_path_buf(), rule.id.clone()))
                } else {
                    policy.local_imports
                }
            });
        if !allowed {
            push_warning(
                warnings,
                ConfigWarning::InvalidRule {
                    id: Some(rule.id.clone()),
                    kind: kind.to_string(),
                    reason: if source.is_some_and(|source| remote_sources.contains(source)) {
                        "shell rules from URL imports require remote_imports, an importing-edge permission, and a matching SHA-256 pin".to_string()
                    } else if !policy.enabled {
                        "native shell rules are disabled by config.shell.enabled".to_string()
                    } else if source.is_none() {
                        "shell rule source is ambiguous; refusing executable code".to_string()
                    } else {
                        "shell rules from local imports are disabled by config.shell.local_imports"
                            .to_string()
                    },
                },
            );
        }
        allowed
    });
}

fn effective_known_rule_types(configured: &BTreeSet<String>) -> BTreeSet<String> {
    let mut known = configured.clone();
    known.extend(crate::rules::shell::known_rule_types());
    known
}

fn find_rule_sources(
    sources: &BTreeSet<PathBuf>,
    rule_ids: &BTreeSet<String>,
) -> BTreeMap<String, RuleSource> {
    let mut candidates = BTreeMap::<String, Vec<RuleSource>>::new();
    for path in sources {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            let Some(id) = declared_rule_id(line) else {
                continue;
            };
            if rule_ids.contains(id) {
                candidates
                    .entry(id.to_string())
                    .or_default()
                    .push(RuleSource {
                        path: path.clone(),
                        line: index + 1,
                    });
            }
        }
    }

    candidates
        .into_iter()
        .filter_map(|(id, locations)| match locations.as_slice() {
            [location] => Some((id, location.clone())),
            _ => None,
        })
        .collect()
}

fn declared_rule_id(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let line = line.strip_prefix("- ").unwrap_or(line).trim_start();
    let value = line
        .strip_prefix("id:")
        .or_else(|| line.strip_prefix("id ="))?
        .trim();
    let value = value
        .split_once('#')
        .map_or(value, |(value, _)| value)
        .trim();
    if let Some(value) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    {
        Some(value)
    } else if let Some(value) = value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
    {
        Some(value)
    } else {
        (!value.is_empty()).then_some(value)
    }
}

fn filter_invalid_rules(
    rules: &mut Vec<RawRule>,
    warnings: &mut Vec<ConfigWarning>,
    known_rule_types: &BTreeSet<String>,
) {
    rules.retain_mut(|rule| {
        let kind = rule.kind.as_deref().unwrap_or("regexp").to_string();
        let id = (!rule.id.trim().is_empty()).then(|| rule.id.clone());
        if id.is_none() {
            push_warning(
                warnings,
                ConfigWarning::InvalidRule {
                    id: None,
                    kind,
                    reason: "rule id cannot be empty".to_string(),
                },
            );
            return false;
        }
        if !is_registered_rule_type(&kind) {
            if !known_rule_types.contains(&kind) {
                push_warning(warnings, ConfigWarning::IgnoredRuleType { kind });
                return false;
            }
            // A discovered plugin provides this type. Shared-field parse
            // failures are dropped now; settings validation happens when the
            // initialized plugin compiles the rule.
            if let Some(reason) = rule.validation_error.as_deref() {
                push_warning(
                    warnings,
                    ConfigWarning::InvalidRule {
                        id,
                        kind,
                        reason: reason.to_string(),
                    },
                );
                return false;
            }
            return true;
        }
        filter_invalid_rules(&mut rule.rules, warnings, known_rule_types);
        // Engine validation only covers the built-in parts here: plugin-typed
        // descendants are stripped because their providers are not available
        // until plugin initialization.
        let stripped = strip_plugin_rules(rule, known_rule_types);
        let deferred_to_plugins = stripped.rules.is_empty() && !rule.rules.is_empty();
        if deferred_to_plugins && kind == "ruleset" {
            return true;
        }
        match RuleEngine::compile(vec![stripped]) {
            Ok(_) => true,
            Err(error) => {
                push_warning(
                    warnings,
                    ConfigWarning::InvalidRule {
                        id,
                        kind,
                        reason: format!("{error:#}"),
                    },
                );
                false
            }
        }
    });
}

/// Clones a rule with plugin-typed descendants removed so the built-in parts
/// can be validated before plugins are initialized.
fn strip_plugin_rules(rule: &RawRule, known_rule_types: &BTreeSet<String>) -> RawRule {
    let mut stripped = rule.clone();
    stripped.rules = stripped
        .rules
        .iter()
        .filter_map(|child| {
            let kind = child.kind.as_deref().unwrap_or("regexp");
            if !is_registered_rule_type(kind) && known_rule_types.contains(kind) {
                return None;
            }
            let stripped_child = strip_plugin_rules(child, known_rule_types);
            // A nested ruleset backed entirely by known plugin rules cannot be
            // compiled until those providers are initialized. Omit that
            // deferred branch from built-in validation instead of presenting
            // it to the engine as an invalid empty ruleset.
            if kind == "ruleset" && !child.rules.is_empty() && stripped_child.rules.is_empty() {
                return None;
            }
            Some(stripped_child)
        })
        .collect();
    stripped
}

fn collect_rule_warnings(
    rules: &[RawRule],
    rule_ids: &mut BTreeSet<String>,
    warnings: &mut Vec<ConfigWarning>,
) {
    for rule in rules {
        if !rule_ids.insert(rule.id.clone()) {
            push_warning(
                warnings,
                ConfigWarning::DuplicateRuleId {
                    id: rule.id.clone(),
                },
            );
        }
        if rule.app_mode == Some(AppMode::Whitelist) && rule.apps.is_empty() {
            push_warning(
                warnings,
                ConfigWarning::EmptyRuleAppWhitelist {
                    id: rule.id.clone(),
                },
            );
        }
        collect_rule_warnings(&rule.rules, rule_ids, warnings);
    }
}

fn push_warning(warnings: &mut Vec<ConfigWarning>, warning: ConfigWarning) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

fn load_yaml(path: &Path, options: &ConfigLoadOptions) -> Result<LoadedConfig> {
    let mut stack = VecDeque::new();
    let mut sources = BTreeSet::new();
    let mut context = LoadContext::new(options.clone());
    let value = load_yaml_value(path, &mut stack, &mut sources, &mut context)?;
    let document = serde_yaml::from_value(value)
        .with_context(|| format!("invalid YAML config {}", path.display()))?;
    let root = normalize_path(path)?;
    let remote_sources = context.remote_sources.clone();
    let authorized_remote_shell_rules = context.authorized_remote_shell_rules.clone();
    Ok(finish_loaded_config(
        document,
        sources,
        context.remote_imports,
        context.warnings,
        &options.known_rule_types,
        Some((&root, &remote_sources, &authorized_remote_shell_rules)),
    ))
}

struct LoadContext {
    options: ConfigLoadOptions,
    import_refresh_interval: Duration,
    loaded: BTreeSet<PathBuf>,
    warnings: Vec<ConfigWarning>,
    remote_sources: BTreeSet<PathBuf>,
    remote_imports: BTreeMap<String, PathBuf>,
    authorized_remote_shell_rules: BTreeSet<(PathBuf, String)>,
    shell_policy: ShellConfig,
}

impl LoadContext {
    fn new(options: ConfigLoadOptions) -> Self {
        Self {
            options,
            import_refresh_interval: Duration::from_secs(
                AppConfig::default().import_refresh_interval,
            ),
            loaded: BTreeSet::new(),
            warnings: Vec::new(),
            remote_sources: BTreeSet::new(),
            remote_imports: BTreeMap::new(),
            authorized_remote_shell_rules: BTreeSet::new(),
            shell_policy: ShellConfig::default(),
        }
    }

    fn warn(&mut self, warning: ConfigWarning) {
        if !self.warnings.contains(&warning) {
            self.warnings.push(warning);
        }
    }
}

fn load_yaml_value(
    path: &Path,
    stack: &mut VecDeque<PathBuf>,
    sources: &mut BTreeSet<PathBuf>,
    context: &mut LoadContext,
) -> Result<YamlValue> {
    let path = normalize_path(path)?;
    if let Some(cycle_start) = stack.iter().position(|source| source == &path) {
        let mut chain = stack.iter().skip(cycle_start).cloned().collect::<Vec<_>>();
        chain.push(path);
        context.warn(ConfigWarning::ImportCycle { chain });
        return Ok(YamlValue::Sequence(Vec::new()));
    }
    // The `loaded` check deduplicates diamond imports (the same file reachable
    // through two import chains) in addition to the stack's cycle protection.
    if !context.loaded.insert(path.clone()) {
        return Ok(YamlValue::Sequence(Vec::new()));
    }

    stack.push_back(path.clone());
    sources.insert(path.clone());
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let value: YamlValue =
        serde_yaml::from_str(&text).with_context(|| format!("parse YAML {}", path.display()))?;
    if stack.len() == 1 {
        context.import_refresh_interval = extract_import_refresh_interval(&value);
        context.shell_policy = extract_yaml_shell_policy(&value)?;
    }
    let expanded = expand_yaml_imports(
        value,
        path.parent().unwrap_or(Path::new(".")),
        stack,
        sources,
        context,
    )
    .with_context(|| format!("expand YAML imports in {}", path.display()))?;
    stack.pop_back();
    Ok(expanded)
}

fn collect_yaml_sources(
    path: &Path,
    stack: &mut VecDeque<PathBuf>,
    sources: &mut BTreeSet<PathBuf>,
    context: &mut LoadContext,
) {
    let path = normalize_path(path).unwrap_or_else(|_| path.to_path_buf());
    if stack.contains(&path) {
        return;
    }
    sources.insert(path.clone());
    stack.push_back(path.clone());

    let Ok(text) = fs::read_to_string(&path) else {
        stack.pop_back();
        return;
    };
    let Ok(value) = serde_yaml::from_str::<YamlValue>(&text) else {
        stack.pop_back();
        return;
    };
    if stack.len() == 1 {
        context.import_refresh_interval = extract_import_refresh_interval(&value);
    }
    collect_yaml_import_sources(
        &value,
        path.parent().unwrap_or(Path::new(".")),
        stack,
        sources,
        context,
    );
    stack.pop_back();
}

fn collect_yaml_import_sources(
    value: &YamlValue,
    base_dir: &Path,
    stack: &mut VecDeque<PathBuf>,
    sources: &mut BTreeSet<PathBuf>,
    context: &mut LoadContext,
) {
    match value {
        YamlValue::Sequence(items) => {
            for item in items {
                collect_yaml_import_sources(item, base_dir, stack, sources, context);
            }
        }
        YamlValue::Mapping(mapping) => {
            if let Some(import) = mapping_import_directive(mapping) {
                if let Ok(path) = resolve_import_path(base_dir, &import.source, context) {
                    collect_yaml_sources(&path, stack, sources, context);
                }
                return;
            }
            if mapping_has_legacy_include(mapping) {
                return;
            }
            for (key, value) in mapping {
                collect_yaml_import_sources(key, base_dir, stack, sources, context);
                collect_yaml_import_sources(value, base_dir, stack, sources, context);
            }
        }
        YamlValue::Tagged(tagged) => {
            collect_yaml_import_sources(&tagged.value, base_dir, stack, sources, context);
        }
        _ => {}
    }
}

fn expand_yaml_imports(
    value: YamlValue,
    base_dir: &Path,
    stack: &mut VecDeque<PathBuf>,
    sources: &mut BTreeSet<PathBuf>,
    context: &mut LoadContext,
) -> Result<YamlValue> {
    match value {
        YamlValue::Sequence(items) => {
            let mut out = Vec::new();
            for item in items {
                let expanded = expand_yaml_imports(item, base_dir, stack, sources, context)?;
                if let YamlValue::Sequence(nested) = expanded {
                    out.extend(nested);
                } else {
                    out.push(expanded);
                }
            }
            Ok(YamlValue::Sequence(out))
        }
        YamlValue::Mapping(mapping) => expand_mapping(mapping, base_dir, stack, sources, context),
        YamlValue::Tagged(tagged) => {
            let tag = tagged.tag.to_string();
            bail!("unsupported YAML tag {tag}; use `import: path-or-url` inside rules instead")
        }
        other => Ok(other),
    }
}

fn expand_mapping(
    mapping: Mapping,
    base_dir: &Path,
    stack: &mut VecDeque<PathBuf>,
    sources: &mut BTreeSet<PathBuf>,
    context: &mut LoadContext,
) -> Result<YamlValue> {
    if mapping
        .get(YamlValue::String("type".to_string()))
        .and_then(YamlValue::as_str)
        .is_some_and(|kind| !is_registered_rule_type(kind))
    {
        return Ok(YamlValue::Mapping(mapping));
    }
    if let Some(import) = mapping_import_directive(&mapping) {
        let path = resolve_import_path(base_dir, &import.source, context)?;
        let rules = load_imported_rules_by_format(&path, stack, sources, context)?;
        authorize_imported_shell_rules(&import, &path, &rules, context)?;
        return serde_yaml::to_value(rules).context("serialize imported rules");
    }
    if mapping_has_legacy_include(&mapping) {
        bail!("YAML `include` is unsupported; use `import: path-or-url` instead");
    }

    let mut out = Mapping::new();
    for (key, value) in mapping {
        let key = expand_yaml_imports(key, base_dir, stack, sources, context)?;
        let value = expand_yaml_imports(value, base_dir, stack, sources, context)?;
        out.insert(key, value);
    }
    Ok(YamlValue::Mapping(out))
}

fn unwrap_imported_yaml(value: YamlValue) -> Result<YamlValue> {
    match value {
        YamlValue::Mapping(mut mapping) => {
            let rules_key = YamlValue::String("rules".to_string());
            if mapping.contains_key(&rules_key) {
                match mapping.remove(&rules_key).unwrap_or(YamlValue::Null) {
                    YamlValue::Sequence(rules) => Ok(YamlValue::Sequence(rules)),
                    other => bail!("imported YAML rules key must contain a list, got {other:?}"),
                }
            } else {
                bail!("imported YAML must be a list of rules or a mapping with a rules key")
            }
        }
        YamlValue::Sequence(_) => Ok(value),
        other => bail!(
            "imported YAML must be a list of rules or a mapping with a rules key, got {other:?}"
        ),
    }
}

fn extract_import_refresh_interval(value: &YamlValue) -> Duration {
    let seconds = value
        .as_mapping()
        .and_then(|mapping| mapping.get(YamlValue::String("config".to_string())))
        .and_then(YamlValue::as_mapping)
        .and_then(|config| config.get(YamlValue::String("import_refresh_interval".to_string())))
        .and_then(YamlValue::as_u64)
        .unwrap_or_else(|| AppConfig::default().import_refresh_interval);
    Duration::from_secs(seconds)
}

fn extract_yaml_shell_policy(value: &YamlValue) -> Result<ShellConfig> {
    value
        .as_mapping()
        .and_then(|mapping| mapping.get(YamlValue::String("config".to_string())))
        .and_then(YamlValue::as_mapping)
        .and_then(|config| config.get(YamlValue::String("shell".to_string())))
        .cloned()
        .map(serde_yaml::from_value)
        .transpose()
        .context("invalid config.shell")
        .map(|policy| policy.unwrap_or_default())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum ImportValue {
    Short(String),
    Expanded(ExpandedImport),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpandedImport {
    source: String,
    #[serde(default)]
    permissions: ImportPermissions,
    #[serde(default)]
    sha256: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ImportPermissions {
    shell: bool,
}

#[derive(Debug, Clone)]
struct ImportDirective {
    source: String,
    shell: bool,
    sha256: Option<String>,
}

fn mapping_import_directive(mapping: &Mapping) -> Option<ImportDirective> {
    if mapping.len() != 1 {
        return None;
    }
    let value = mapping
        .get(YamlValue::String("import".to_string()))
        .cloned()?;
    let value = serde_yaml::from_value::<ImportValue>(value).ok()?;
    Some(match value {
        ImportValue::Short(source) => ImportDirective {
            source,
            shell: false,
            sha256: None,
        },
        ImportValue::Expanded(import) => ImportDirective {
            source: import.source,
            shell: import.permissions.shell,
            sha256: import.sha256,
        },
    })
}

fn mapping_has_legacy_include(mapping: &Mapping) -> bool {
    mapping.len() == 1 && mapping.contains_key(YamlValue::String("include".to_string()))
}

fn authorize_imported_shell_rules(
    import: &ImportDirective,
    path: &Path,
    rules: &[RawRule],
    context: &mut LoadContext,
) -> Result<()> {
    if !context.remote_sources.contains(path) || !import.shell {
        return Ok(());
    }
    if !context.shell_policy.enabled || !context.shell_policy.remote_imports {
        bail!(
            "remote import {:?} requests shell permission, but config.shell.enabled and remote_imports must both be true",
            import.source
        );
    }
    let expected = import
        .sha256
        .as_deref()
        .context("remote shell import requires sha256")?
        .trim()
        .strip_prefix("sha256:")
        .unwrap_or_else(|| import.sha256.as_deref().unwrap().trim())
        .to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("remote shell import sha256 must contain exactly 64 hexadecimal digits");
    }
    let bytes = fs::read(path).with_context(|| format!("read pinned import {}", path.display()))?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != expected {
        bail!(
            "remote shell import SHA-256 mismatch for {:?}: expected {}, got {}",
            import.source,
            expected,
            actual
        );
    }
    collect_shell_rule_ids(rules, &mut |id| {
        context
            .authorized_remote_shell_rules
            .insert((path.to_path_buf(), id.to_string()));
    });
    Ok(())
}

fn collect_shell_rule_ids(rules: &[RawRule], found: &mut impl FnMut(&str)) {
    for rule in rules {
        if matches!(rule.kind.as_deref(), Some("shell" | "item-shell")) {
            found(&rule.id);
        }
        collect_shell_rule_ids(&rule.rules, found);
    }
}

fn resolve_import_path(
    base_dir: &Path,
    import_path: &str,
    context: &mut LoadContext,
) -> Result<PathBuf> {
    if let Ok(url) = Url::parse(import_path) {
        // Single-letter schemes are Windows drive letters (`C:\rules.toml`),
        // not URL schemes; fall through to filesystem path resolution.
        if url.scheme().len() > 1 {
            return match url.scheme() {
                "file" => url
                    .to_file_path()
                    .map_err(|_| anyhow!("invalid file import URL {import_path:?}"))
                    .and_then(|path| require_import_file(path, import_path)),
                "http" | "https" => {
                    let path = resolve_url_import(normalize_import_url(url), context)?;
                    context.remote_sources.insert(path.clone());
                    Ok(path)
                }
                scheme => bail!("unsupported import URL scheme {scheme:?} in {import_path:?}"),
            };
        }
    }

    require_import_file(resolve_file_import_path(base_dir, import_path), import_path)
}

fn resolve_file_import_path(base_dir: &Path, import_path: &str) -> PathBuf {
    let path = PathBuf::from(import_path);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

fn require_import_file(path: PathBuf, import_path: &str) -> Result<PathBuf> {
    if !path.exists() {
        bail!(
            "import file {import_path:?} does not exist at {}",
            path.display()
        );
    }
    if !path.is_file() {
        bail!("import {import_path:?} is not a file at {}", path.display());
    }
    normalize_path(&path)
}

fn resolve_url_import(url: Url, context: &mut LoadContext) -> Result<PathBuf> {
    let cache_dir = context
        .options
        .state_dir
        .as_ref()
        .context("URL imports require a state_dir")?
        .join("url-imports");
    fs::create_dir_all(&cache_dir).with_context(|| format!("create {}", cache_dir.display()))?;
    let cache_path = cache_dir.join(url_cache_file_name(&url));
    if context.options.refresh_url_imports
        && should_refresh_url_import(&cache_path, context.import_refresh_interval)
    {
        if let Err(error) = refresh_url_import(&url, &cache_path) {
            if cache_path.exists() {
                crate::logging::event(format!(
                    "URL import refresh failed for {}; using cached copy: {error:#}",
                    url.as_str()
                ));
            } else {
                return Err(error);
            }
        }
    }
    if !cache_path.exists() {
        bail!(
            "URL import {} has no cached file and downloading is disabled",
            url.as_str()
        );
    }
    let cache_path = normalize_path(&cache_path)?;
    context
        .remote_imports
        .insert(url.as_str().to_owned(), cache_path.clone());
    Ok(cache_path)
}

fn normalize_import_url(url: Url) -> Url {
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return url;
    };

    match host.as_str() {
        "github.com" => normalize_github_import_url(&url).unwrap_or(url),
        "gist.github.com" => normalize_gist_import_url(&url).unwrap_or(url),
        "gitlab.com" => normalize_gitlab_import_url(&url).unwrap_or(url),
        "pastebin.com" | "www.pastebin.com" => normalize_pastebin_import_url(&url).unwrap_or(url),
        "rentry.co" | "www.rentry.co" => normalize_rentry_import_url(&url).unwrap_or(url),
        "hastebin.com" | "www.hastebin.com" | "hastebin.io" | "hastebin.skyra.pw" => {
            normalize_hastebin_import_url(&url).unwrap_or(url)
        }
        "dpaste.org" | "www.dpaste.org" => normalize_dpaste_import_url(&url).unwrap_or(url),
        "bitbucket.org" => normalize_bitbucket_import_url(&url).unwrap_or(url),
        "codeberg.org" | "gitea.com" => normalize_gitea_import_url(&url).unwrap_or(url),
        _ => url,
    }
}

fn normalize_github_import_url(url: &Url) -> Option<Url> {
    let segments = url_path_segments(url)?;
    if segments.len() < 5 || segments[2] != "blob" {
        return None;
    }
    Url::parse(&format!(
        "https://raw.githubusercontent.com/{}/{}/{}/{}",
        segments[0],
        segments[1],
        segments[3],
        segments[4..].join("/")
    ))
    .ok()
}

fn normalize_gist_import_url(url: &Url) -> Option<Url> {
    let segments = url_path_segments(url)?;
    if segments.len() < 2 || segments.contains(&"raw") {
        return None;
    }
    let mut raw = format!(
        "https://gist.githubusercontent.com/{}/{}/raw",
        segments[0], segments[1]
    );
    if segments.len() > 2 {
        raw.push('/');
        raw.push_str(&segments[2..].join("/"));
    }
    Url::parse(&raw).ok()
}

fn normalize_gitlab_import_url(url: &Url) -> Option<Url> {
    let mut segments = url_path_segments(url)?;
    let marker = segments.iter().position(|segment| *segment == "-")?;
    let kind = segments.get(marker + 1)?;
    match *kind {
        "blob" => {
            segments[marker + 1] = "raw";
            let mut raw = url.clone();
            raw.set_path(&segments.join("/"));
            Some(raw)
        }
        "snippets" if !segments.contains(&"raw") => {
            segments.truncate(marker + 3);
            segments.push("raw");
            let mut raw = url.clone();
            raw.set_path(&segments.join("/"));
            Some(raw)
        }
        _ => None,
    }
}

fn normalize_pastebin_import_url(url: &Url) -> Option<Url> {
    let segments = url_path_segments(url)?;
    if segments.len() != 1 || segments[0] == "raw" {
        return None;
    }
    Url::parse(&format!("https://pastebin.com/raw/{}", segments[0])).ok()
}

fn normalize_rentry_import_url(url: &Url) -> Option<Url> {
    let segments = url_path_segments(url)?;
    if segments.len() != 1 {
        return None;
    }
    let mut raw = url.clone();
    raw.set_path(&format!("{}/raw", segments[0]));
    Some(raw)
}

fn normalize_hastebin_import_url(url: &Url) -> Option<Url> {
    let segments = url_path_segments(url)?;
    let paste_id = match segments.as_slice() {
        ["raw", ..] | ["documents", ..] => return None,
        ["share", id] | [id] => *id,
        _ => return None,
    };
    let mut raw = url.clone();
    raw.set_path(&format!("raw/{paste_id}"));
    Some(raw)
}

fn normalize_dpaste_import_url(url: &Url) -> Option<Url> {
    let segments = url_path_segments(url)?;
    if segments.len() != 1 {
        return None;
    }
    let mut raw = url.clone();
    raw.set_path(&format!("{}/raw", segments[0]));
    Some(raw)
}

fn normalize_bitbucket_import_url(url: &Url) -> Option<Url> {
    let mut segments = url_path_segments(url)?;
    if segments.len() < 5 || segments[2] != "src" {
        return None;
    }
    segments[2] = "raw";
    let mut raw = url.clone();
    raw.set_path(&segments.join("/"));
    Some(raw)
}

fn normalize_gitea_import_url(url: &Url) -> Option<Url> {
    let mut segments = url_path_segments(url)?;
    if segments.len() < 6 || segments[2] != "src" || segments[3] != "branch" {
        return None;
    }
    segments[2] = "raw";
    let mut raw = url.clone();
    raw.set_path(&segments.join("/"));
    Some(raw)
}

fn url_path_segments(url: &Url) -> Option<Vec<&str>> {
    Some(
        url.path_segments()?
            .filter(|segment| !segment.is_empty())
            .collect(),
    )
}

fn should_refresh_url_import(cache_path: &Path, interval: Duration) -> bool {
    if interval.is_zero() {
        return false;
    }
    if !cache_path.exists() {
        return true;
    }
    let refresh_marker = url_cache_metadata_path(cache_path);
    let Ok(metadata) = fs::metadata(&refresh_marker).or_else(|_| fs::metadata(cache_path)) else {
        return true;
    };
    let Ok(modified) = metadata.modified() else {
        return true;
    };
    SystemTime::now()
        .duration_since(modified)
        .map_or(true, |age| age >= interval)
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct UrlCacheMetadata {
    etag: Option<String>,
    last_modified: Option<String>,
}

#[cfg(feature = "desktop")]
pub(crate) fn refresh_remote_imports(remote_imports: &BTreeMap<String, PathBuf>) -> Result<bool> {
    let mut changed = false;
    for (url, cache_path) in remote_imports {
        let url = Url::parse(url).with_context(|| format!("parse cached import URL {url}"))?;
        match refresh_url_import(&url, cache_path) {
            Ok(import_changed) => changed |= import_changed,
            Err(error) if cache_path.exists() => crate::logging::event(format!(
                "URL import refresh failed for {}; using cached copy: {error:#}",
                url.as_str()
            )),
            Err(error) => return Err(error),
        }
    }
    Ok(changed)
}

fn refresh_url_import(url: &Url, cache_path: &Path) -> Result<bool> {
    let tmp_path = cache_path.with_extension("tmp");
    let metadata_path = url_cache_metadata_path(cache_path);
    let metadata = if cache_path.exists() {
        fs::read(&metadata_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<UrlCacheMetadata>(&bytes).ok())
            .unwrap_or_default()
    } else {
        UrlCacheMetadata::default()
    };
    let validators = crate::platform::download::HttpValidators {
        etag: metadata.etag,
        last_modified: metadata.last_modified,
    };
    let outcome = crate::platform::download::download_to_file_conditional(
        url.as_str(),
        &tmp_path,
        Duration::from_secs(30),
        None,
        Some(&validators),
    )
    .with_context(|| format!("download URL import {}", url.as_str()))?;
    let (changed, validators) = match outcome {
        crate::platform::download::DownloadOutcome::NotModified(validators) => {
            if !cache_path.exists() {
                bail!(
                    "URL import {} returned not modified without a cached file",
                    url.as_str()
                );
            }
            (false, validators)
        }
        crate::platform::download::DownloadOutcome::Updated(validators) => {
            if files_have_same_contents(&tmp_path, cache_path)? {
                fs::remove_file(&tmp_path)
                    .with_context(|| format!("remove unchanged {}", tmp_path.display()))?;
                (false, validators)
            } else {
                fs::rename(&tmp_path, cache_path)
                    .with_context(|| format!("write URL import cache {}", cache_path.display()))?;
                (true, validators)
            }
        }
    };
    let metadata = UrlCacheMetadata {
        etag: validators.etag,
        last_modified: validators.last_modified,
    };
    if let Err(error) = write_url_cache_metadata(&metadata_path, &metadata) {
        crate::logging::event(format!(
            "URL import refresh metadata unavailable for {}: {error:#}",
            url.as_str()
        ));
    }
    Ok(changed)
}

fn files_have_same_contents(left: &Path, right: &Path) -> Result<bool> {
    let Ok(right_metadata) = fs::metadata(right) else {
        return Ok(false);
    };
    if fs::metadata(left)?.len() != right_metadata.len() {
        return Ok(false);
    }
    let mut left = fs::File::open(left)?;
    let mut right = fs::File::open(right)?;
    let mut left_buffer = [0_u8; 16 * 1024];
    let mut right_buffer = [0_u8; 16 * 1024];
    loop {
        let left_read = left.read(&mut left_buffer)?;
        let right_read = right.read(&mut right_buffer)?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

fn url_cache_metadata_path(cache_path: &Path) -> PathBuf {
    let mut name = cache_path.as_os_str().to_os_string();
    name.push(".http.json");
    PathBuf::from(name)
}

fn write_url_cache_metadata(path: &Path, metadata: &UrlCacheMetadata) -> Result<()> {
    let bytes = serde_json::to_vec(metadata).context("serialize URL import metadata")?;
    // This is only an HTTP optimization hint, not source-of-truth config.
    // A partial/corrupt write is ignored on the next refresh and is safer than
    // relying on platform-specific rename-over-existing behavior.
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

fn url_cache_file_name(url: &Url) -> String {
    let encoded = url
        .as_str()
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let ext = Path::new(url.path())
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| !ext.is_empty());
    ext.map_or(encoded.clone(), |ext| format!("{encoded}.{ext}"))
}

fn load_toml(path: &Path, options: &ConfigLoadOptions) -> Result<LoadedConfig> {
    let mut stack = VecDeque::new();
    let mut sources = BTreeSet::new();
    let mut context = LoadContext::new(options.clone());
    let document = load_toml_document(path, &mut stack, &mut sources, &mut context)?;
    let root = normalize_path(path)?;
    let remote_sources = context.remote_sources.clone();
    let authorized_remote_shell_rules = context.authorized_remote_shell_rules.clone();
    Ok(finish_loaded_config(
        document,
        sources,
        context.remote_imports,
        context.warnings,
        &options.known_rule_types,
        Some((&root, &remote_sources, &authorized_remote_shell_rules)),
    ))
}

fn load_toml_document(
    path: &Path,
    stack: &mut VecDeque<PathBuf>,
    sources: &mut BTreeSet<PathBuf>,
    context: &mut LoadContext,
) -> Result<ConfigDocument> {
    let path = normalize_path(path)?;
    if let Some(cycle_start) = stack.iter().position(|source| source == &path) {
        let mut chain = stack.iter().skip(cycle_start).cloned().collect::<Vec<_>>();
        chain.push(path);
        context.warn(ConfigWarning::ImportCycle { chain });
        return Ok(ConfigDocument::default());
    }
    if !context.loaded.insert(path.clone()) {
        return Ok(ConfigDocument::default());
    }

    stack.push_back(path.clone());
    sources.insert(path.clone());
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let value: toml::Value =
        toml::from_str(&text).with_context(|| format!("parse TOML {}", path.display()))?;
    if stack.len() == 1 {
        context.import_refresh_interval = extract_toml_import_refresh_interval(&value);
        context.shell_policy = value
            .get("config")
            .and_then(|config| config.get("shell"))
            .cloned()
            .map(toml::Value::try_into)
            .transpose()
            .context("invalid config.shell")?
            .unwrap_or_default();
    }
    let document = toml_document_from_value(
        value,
        path.parent().unwrap_or(Path::new(".")),
        stack,
        sources,
        context,
    )
    .with_context(|| format!("expand TOML imports in {}", path.display()))?;
    stack.pop_back();
    Ok(document)
}

fn toml_document_from_value(
    value: toml::Value,
    base_dir: &Path,
    stack: &mut VecDeque<PathBuf>,
    sources: &mut BTreeSet<PathBuf>,
    context: &mut LoadContext,
) -> Result<ConfigDocument> {
    let config = value
        .get("config")
        .cloned()
        .map(toml::Value::try_into)
        .transpose()
        .context("invalid TOML config section")?
        .unwrap_or_default();
    // Imported documents contribute rules only; their `plugins` sections are
    // ignored like their `config` sections.
    let plugins = if stack.len() == 1 {
        value
            .get("plugins")
            .cloned()
            .map(toml::Value::try_into)
            .transpose()
            .context("invalid TOML plugins section")?
            .unwrap_or_default()
    } else {
        BTreeMap::new()
    };
    let rules = expand_toml_rules(&value, base_dir, stack, sources, context)?;
    Ok(ConfigDocument {
        config,
        rules,
        plugins,
    })
}

fn collect_toml_sources(
    path: &Path,
    stack: &mut VecDeque<PathBuf>,
    sources: &mut BTreeSet<PathBuf>,
    context: &mut LoadContext,
) {
    let path = normalize_path(path).unwrap_or_else(|_| path.to_path_buf());
    if stack.contains(&path) {
        return;
    }
    sources.insert(path.clone());
    stack.push_back(path.clone());

    let Ok(text) = fs::read_to_string(&path) else {
        stack.pop_back();
        return;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&text) else {
        stack.pop_back();
        return;
    };
    if stack.len() == 1 {
        context.import_refresh_interval = extract_toml_import_refresh_interval(&value);
    }
    collect_toml_import_sources(
        &value,
        path.parent().unwrap_or(Path::new(".")),
        stack,
        sources,
        context,
    );
    stack.pop_back();
}

fn collect_toml_import_sources(
    value: &toml::Value,
    base_dir: &Path,
    stack: &mut VecDeque<PathBuf>,
    sources: &mut BTreeSet<PathBuf>,
    context: &mut LoadContext,
) {
    let Some(rules) = value.get("rules").and_then(toml::Value::as_array) else {
        return;
    };

    for rule in rules {
        if let Some(import) = toml_import_directive(rule) {
            if let Ok(path) = resolve_import_path(base_dir, &import.source, context) {
                collect_import_sources_by_format(&path, stack, sources, context);
            }
        }
    }
}

fn collect_import_sources_by_format(
    path: &Path,
    stack: &mut VecDeque<PathBuf>,
    sources: &mut BTreeSet<PathBuf>,
    context: &mut LoadContext,
) {
    match imported_config_format(path) {
        Ok(ConfigFormat::Yaml) => collect_yaml_sources(path, stack, sources, context),
        Ok(ConfigFormat::Toml) => collect_toml_sources(path, stack, sources, context),
        Err(_) => {
            sources.insert(path.to_path_buf());
        }
    }
}

fn expand_toml_rules(
    value: &toml::Value,
    base_dir: &Path,
    stack: &mut VecDeque<PathBuf>,
    sources: &mut BTreeSet<PathBuf>,
    context: &mut LoadContext,
) -> Result<Vec<RawRule>> {
    let Some(rules) = value.get("rules").and_then(toml::Value::as_array) else {
        return Ok(Vec::new());
    };

    let mut expanded = Vec::new();
    for rule in rules {
        if let Some(import) = toml_import_directive(rule) {
            let path = resolve_import_path(base_dir, &import.source, context)?;
            let imported = load_imported_rules_by_format(&path, stack, sources, context)?;
            authorize_imported_shell_rules(&import, &path, &imported, context)?;
            expanded.extend(imported);
        } else if toml_has_legacy_include(rule) {
            bail!("TOML `include` is unsupported; use `import = \"path-or-url\"` inside rules instead");
        } else {
            expanded.push(rule.clone().try_into().context("invalid TOML rule")?);
        }
    }
    Ok(expanded)
}

fn load_imported_rules_by_format(
    path: &Path,
    stack: &mut VecDeque<PathBuf>,
    sources: &mut BTreeSet<PathBuf>,
    context: &mut LoadContext,
) -> Result<Vec<RawRule>> {
    match imported_config_format(path)? {
        ConfigFormat::Yaml => {
            let included = load_yaml_value(path, stack, sources, context)?;
            let value = unwrap_imported_yaml(included)?;
            Ok(serde_yaml::from_value(value)
                .with_context(|| format!("invalid imported YAML rules {}", path.display()))?)
        }
        ConfigFormat::Toml => Ok(load_toml_document(path, stack, sources, context)?.rules),
    }
}

fn toml_import_directive(value: &toml::Value) -> Option<ImportDirective> {
    let table = value.as_table()?;
    if table.len() != 1 {
        return None;
    }
    let import = table.get("import")?;
    if let Some(source) = import.as_str() {
        return Some(ImportDirective {
            source: source.to_string(),
            shell: false,
            sha256: None,
        });
    }
    let expanded: ExpandedImport = import.clone().try_into().ok()?;
    Some(ImportDirective {
        source: expanded.source,
        shell: expanded.permissions.shell,
        sha256: expanded.sha256,
    })
}

fn toml_has_legacy_include(value: &toml::Value) -> bool {
    value
        .as_table()
        .is_some_and(|table| table.len() == 1 && table.contains_key("include"))
}

fn extract_toml_import_refresh_interval(value: &toml::Value) -> Duration {
    let seconds = value
        .get("config")
        .and_then(|config| config.get("import_refresh_interval"))
        .and_then(toml::Value::as_integer)
        .and_then(|seconds| u64::try_from(seconds).ok())
        .unwrap_or_else(|| AppConfig::default().import_refresh_interval);
    Duration::from_secs(seconds)
}

fn normalize_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        path.canonicalize()
            .with_context(|| format!("canonicalize {}", path.display()))
    } else {
        Ok(path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    fn assert_normalized_import_url(input: &str, expected: &str) {
        let url = Url::parse(input).unwrap();
        assert_eq!(normalize_import_url(url).as_str(), expected);
    }

    #[test]
    fn import_url_normalization_supports_known_raw_hosts() {
        assert_normalized_import_url(
            "https://github.com/owner/repo/blob/main/rules.yaml",
            "https://raw.githubusercontent.com/owner/repo/main/rules.yaml",
        );
        assert_normalized_import_url(
            "https://gist.github.com/owner/0123456789abcdef0123456789abcdef",
            "https://gist.githubusercontent.com/owner/0123456789abcdef0123456789abcdef/raw",
        );
        assert_normalized_import_url(
            "https://gitlab.com/owner/repo/-/blob/main/rules.yaml",
            "https://gitlab.com/owner/repo/-/raw/main/rules.yaml",
        );
        assert_normalized_import_url(
            "https://gitlab.com/-/snippets/123456",
            "https://gitlab.com/-/snippets/123456/raw",
        );
        assert_normalized_import_url(
            "https://pastebin.com/abc123",
            "https://pastebin.com/raw/abc123",
        );
        assert_normalized_import_url(
            "https://rentry.co/rules-yaml",
            "https://rentry.co/rules-yaml/raw",
        );
        assert_normalized_import_url(
            "https://hastebin.com/share/abc123",
            "https://hastebin.com/raw/abc123",
        );
        assert_normalized_import_url(
            "https://hastebin.skyra.pw/abc123",
            "https://hastebin.skyra.pw/raw/abc123",
        );
        assert_normalized_import_url("https://dpaste.org/abc123", "https://dpaste.org/abc123/raw");
        assert_normalized_import_url("https://paste.rs/abc123", "https://paste.rs/abc123");
        assert_normalized_import_url("https://0x0.st/abc.yaml", "https://0x0.st/abc.yaml");
        assert_normalized_import_url("https://ttm.sh/abc.yaml", "https://ttm.sh/abc.yaml");
        assert_normalized_import_url(
            "https://bitbucket.org/owner/repo/src/main/rules.yaml",
            "https://bitbucket.org/owner/repo/raw/main/rules.yaml",
        );
        assert_normalized_import_url(
            "https://codeberg.org/owner/repo/src/branch/main/rules.yaml",
            "https://codeberg.org/owner/repo/raw/branch/main/rules.yaml",
        );
    }

    #[test]
    fn unchanged_url_import_keeps_cache_contents_and_records_validators() {
        let body = b"rules:\n  - id: unchanged\n    from: cat\n    to: dog\n";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"rules-v1\"\r\nLast-Modified: Wed, 29 Jul 2026 12:00:00 GMT\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });
        let directory = tempfile::tempdir().unwrap();
        let cache_path = directory.path().join("rules.yaml");
        fs::write(&cache_path, body).unwrap();
        let url = Url::parse(&format!("http://{address}/rules.yaml")).unwrap();

        assert!(!refresh_url_import(&url, &cache_path).unwrap());

        assert_eq!(fs::read(&cache_path).unwrap(), body);
        assert!(!cache_path.with_extension("tmp").exists());
        let metadata: UrlCacheMetadata =
            serde_json::from_slice(&fs::read(url_cache_metadata_path(&cache_path)).unwrap())
                .unwrap();
        assert_eq!(metadata.etag.as_deref(), Some("\"rules-v1\""));
        assert_eq!(
            metadata.last_modified.as_deref(),
            Some("Wed, 29 Jul 2026 12:00:00 GMT")
        );
        server.join().unwrap();
    }
}
