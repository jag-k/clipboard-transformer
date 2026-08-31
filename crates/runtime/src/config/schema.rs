use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use schemars::{generate::SchemaSettings, JsonSchema, Schema};
use serde::Serialize;
use serde_json::{json, Value};

use super::AppConfig;
use crate::plugins::PluginConfig;
use ct_core::{AppMode, RulesetMode, UrlTransform, REGISTERED_RULE_TYPES};

#[derive(Debug, Default, Serialize, JsonSchema)]
#[serde(default)]
struct ConfigDocumentSchema {
    config: AppConfig,
    rules: Vec<ConfigRuleSchema>,
    /// Per-plugin permissions and settings, keyed by plugin id.
    plugins: BTreeMap<String, PluginConfig>,
}

/// Top-level and nested rule entries. Assembled from import + each registered handler + unknown.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)]
enum ConfigRuleSchema {
    Import(ImportRuleSchema),
    Regexp(RegexpRuleSchema),
    Url(UrlRuleSchema),
    UrlCleanup(UrlCleanupRuleSchema),
    Ruleset(RulesetRuleSchema),
    Shell(ShellRuleSchema),
    ItemShell(ItemShellRuleSchema),
    Unknown(UnknownRuleSchema),
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
enum UrlRuleType {
    Url,
}

/// Structural `url` handler fields.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct UrlRuleSchema {
    #[serde(rename = "type")]
    kind: UrlRuleType,
    #[serde(flatten)]
    common: RuleCommonSchema,
    /// Optional notification body shown when this rule rewrites clipboard content.
    #[serde(default)]
    message: Option<String>,
    /// URL hosts this rule applies to. Empty means all hosts.
    #[serde(default)]
    hosts: Vec<String>,
    /// One structural URL-to-URL operation.
    transform: UrlTransform,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ImportRuleSchema {
    /// Path, file: URL, http: URL, or https: URL to import. Extensionless imports are parsed as YAML first, then TOML. Only imported rules are used; an imported config section is intentionally ignored. Known page URLs for GitHub files, GitHub Gists, GitLab files, GitLab snippets, Pastebin, Rentry, Hastebin, dpaste.org, Bitbucket files, and Codeberg/Gitea files are automatically converted to raw download URLs. Direct paste.rs, 0x0.st, and ttm.sh links are supported as regular URL imports.
    import: ImportSchemaValue,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)]
enum ImportSchemaValue {
    Short(String),
    Expanded(ExpandedImportSchema),
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ExpandedImportSchema {
    source: String,
    /// Host-owned permissions granted by this trusted importing edge.
    #[serde(default)]
    permissions: ImportPermissionsSchema,
    /// Required SHA-256 pin when a URL import is allowed to contribute shell rules.
    #[serde(default)]
    sha256: Option<String>,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[allow(dead_code)]
struct ImportPermissionsSchema {
    /// Allows pinned shell/item-shell rules from this import.
    shell: bool,
}

/// Shared fields available on every concrete rule handler.
#[derive(Debug, Serialize, JsonSchema)]
#[allow(dead_code)]
struct RuleCommonSchema {
    /// Stable rule identifier. Required and used by notifications, undo, and disable state.
    #[schemars(length(min = 1))]
    id: String,
    /// Optional short display label for notifications.
    #[serde(default)]
    name: Option<String>,
    /// Ordered input format priority for text transforms; activation filter for item transforms. Empty means text. Common aliases are text/plain-text, url, html, rtf, and file/file-url; native format ids are accepted directly.
    #[serde(default)]
    formats: Vec<String>,
    /// Source applications this rule filters on. Values match bundle id or app name.
    #[serde(default)]
    apps: Vec<String>,
    /// How to interpret apps: blacklist skips listed apps; whitelist only allows listed apps.
    #[serde(default)]
    app_mode: Option<AppMode>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
enum RegexpRuleType {
    #[default]
    Regexp,
}

/// `regexp` handler fields. `type` may be omitted; the runtime defaults to regexp.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct RegexpRuleSchema {
    /// Rule kind. Defaults to "regexp" when omitted.
    #[serde(default, rename = "type")]
    kind: RegexpRuleType,
    #[serde(flatten)]
    common: RuleCommonSchema,
    /// Regex pattern for regexp rules.
    from: String,
    /// Replacement template for regexp rules.
    to: String,
    /// Optional regexp flags: i (case-insensitive), m (multi-line), s (dot matches newline), U (swap greed), x (ignore whitespace), or u (Unicode).
    #[serde(default)]
    flags: Option<String>,
    /// Optional notification body template. Regex captures like $1 and $name are expanded.
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
enum UrlCleanupRuleType {
    UrlCleanup,
}

/// `url-cleanup` handler fields.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct UrlCleanupRuleSchema {
    #[serde(rename = "type")]
    kind: UrlCleanupRuleType,
    #[serde(flatten)]
    common: RuleCommonSchema,
    /// Optional notification body shown when this rule rewrites clipboard content.
    #[serde(default)]
    message: Option<String>,
    /// URL hosts this rule applies to. Empty means all hosts.
    #[serde(default)]
    hosts: Vec<String>,
    /// Exact query parameter names to remove from parsed URLs.
    #[serde(default)]
    remove_query_params: Vec<String>,
    /// Query parameter prefixes to remove from parsed URLs.
    #[serde(default)]
    remove_query_prefixes: Vec<String>,
    /// Regex patterns for query parameter names to remove from parsed URLs.
    #[serde(default)]
    remove_query_param_patterns: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
enum RulesetRuleType {
    Ruleset,
}

/// `ruleset` handler fields. Nested `rules` use the same entry schema as top-level rules.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct RulesetRuleSchema {
    #[serde(rename = "type")]
    kind: RulesetRuleType,
    #[serde(flatten)]
    common: RuleCommonSchema,
    /// Nested ruleset application mode.
    #[serde(default)]
    mode: Option<RulesetMode>,
    /// Nested rules for ruleset rules.
    #[serde(default)]
    #[schemars(length(min = 1))]
    rules: Vec<ConfigRuleSchema>,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
enum ShellRuleType {
    Shell,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
enum ItemShellRuleType {
    ItemShell,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)]
enum ShellTimeoutSchema {
    Seconds(u64),
    Duration(String),
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ShellRuleSchema {
    #[serde(rename = "type")]
    kind: ShellRuleType,
    #[serde(flatten)]
    common: RuleCommonSchema,
    /// Inline script source. Exactly one of run or script_path is required.
    #[serde(default)]
    run: Option<String>,
    /// Script file. Relative paths resolve from the declaring config file.
    #[serde(default)]
    script_path: Option<PathBuf>,
    /// Shell name or command template containing `{0}`. Defaults to the user's system shell.
    #[serde(default)]
    shell: Option<String>,
    /// Integer seconds or a duration ending in ms, s, or m. Defaults to 5s.
    #[serde(default)]
    timeout: Option<ShellTimeoutSchema>,
    /// Rule-local environment. CT_* and PWD are reserved by the host.
    #[serde(default)]
    env: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ItemShellRuleSchema {
    #[serde(rename = "type")]
    kind: ItemShellRuleType,
    #[serde(flatten)]
    common: RuleCommonSchema,
    /// Inline script source. Exactly one of run or script_path is required.
    #[serde(default)]
    run: Option<String>,
    /// Script file. Relative paths resolve from the declaring config file.
    #[serde(default)]
    script_path: Option<PathBuf>,
    /// Shell name or command template containing `{0}`. Defaults to the user's system shell.
    #[serde(default)]
    shell: Option<String>,
    /// Integer seconds or a duration ending in ms, s, or m. Defaults to 5s.
    #[serde(default)]
    timeout: Option<ShellTimeoutSchema>,
    /// Rule-local environment. CT_* and PWD are reserved by the host.
    #[serde(default)]
    env: BTreeMap<String, String>,
}

/// Unknown `type` values are ignored by the runtime validator; allow any extra fields.
#[derive(Debug, Serialize, JsonSchema)]
#[allow(dead_code)]
struct UnknownRuleSchema {
    /// Stable rule identifier. Required and used by notifications, undo, and disable state.
    #[schemars(length(min = 1))]
    id: String,
    /// Unrecognized rule kind. Must be a string other than a registered handler type.
    #[serde(rename = "type")]
    kind: String,
}

pub fn json_schema() -> Schema {
    // The published schemas and our enrichment pointers use draft-07's
    // `definitions` vocabulary. A dependency update must not silently migrate
    // that external contract to a newer JSON Schema draft.
    SchemaSettings::draft07()
        .into_generator()
        .into_root_schema_for::<ConfigDocumentSchema>()
}

pub fn json_schema_pretty() -> Result<String> {
    json_schema_pretty_with_plugins(&[])
}

/// One discovered plugin rule type contributed to the effective schema.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginRuleSchemaContribution {
    /// Full namespaced rule type, e.g. `dev.jag-k.gitlab/human-readable-link`.
    pub rule_type: String,
    pub description: Option<String>,
    /// Optional JSON Schema for the rule's settings fields.
    pub settings_schema: Option<Value>,
}

/// Builds schema contributions from every valid discovered manifest without
/// executing plugin code.
pub fn plugin_schema_contributions(
    catalog: &crate::plugins::PluginCatalog,
) -> Vec<PluginRuleSchemaContribution> {
    let mut contributions = Vec::new();
    for entry in &catalog.entries {
        let Ok(manifest) = &entry.manifest else {
            continue;
        };
        for rule in &manifest.rules {
            contributions.push(PluginRuleSchemaContribution {
                rule_type: ct_plugin_api::namespaced_rule_type(&manifest.id, &rule.rule_type),
                description: rule.description.clone().or_else(|| rule.name.clone()),
                settings_schema: rule.settings_schema.clone(),
            });
        }
    }
    contributions.sort_by(|a, b| a.rule_type.cmp(&b.rule_type));
    contributions.dedup_by(|a, b| a.rule_type == b.rule_type);
    contributions
}

/// The effective runtime schema: built-in variants plus one variant per
/// discovered plugin rule type.
pub fn json_schema_pretty_with_plugins(plugins: &[PluginRuleSchemaContribution]) -> Result<String> {
    let mut schema = json_schema().to_value();
    normalize_draft07_schema(&mut schema);
    enrich_config_schema(&mut schema);
    add_plugin_rule_variants(&mut schema, plugins);
    Ok(serde_json::to_string_pretty(&schema)?)
}

/// Mirrors `tools/codegen/src/schema_compat.rs` for the schema generated by the
/// installed application. These rewrites preserve Schemars 0.8 serialization;
/// they do not change which configuration documents the schema accepts.
fn normalize_draft07_schema(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                normalize_draft07_schema(value);
            }
        }
        Value::Object(object) => {
            for value in object.values_mut() {
                normalize_draft07_schema(value);
            }

            // Equivalent single-value constraint; retain the published spelling.
            if let Some(Value::String(constant)) = object.remove("const") {
                object.insert("enum".to_string(), json!([constant]));
            }
            // Ignore formatter-dependent Rustdoc wrapping, not paragraph breaks.
            if let Some(Value::String(description)) = object.get_mut("description") {
                *description = description
                    .split("\n\n")
                    .map(|paragraph| paragraph.split_whitespace().collect::<Vec<_>>().join(" "))
                    .collect::<Vec<_>>()
                    .join("\n\n");
            }
            // `required` is a set in JSON Schema, so ordering is non-semantic.
            if let Some(Value::Array(required)) = object.get_mut("required") {
                required.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
            }
            // 0 and 0.0 are the same JSON number; keep the previous byte form.
            if object.get("minimum").and_then(Value::as_u64) == Some(0) {
                object.insert("minimum".to_string(), json!(0.0));
            }
        }
        _ => {}
    }
}

fn enrich_config_schema(schema: &mut Value) {
    enrich_import_schema(schema);
    enrich_rule_schema(schema);
}

fn enrich_import_schema(schema: &mut Value) {
    let Some(import) = schema
        .pointer_mut("/definitions/ImportRuleSchema/properties/import")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    import.insert(
        "description".to_string(),
        Value::String(
            "Path, file: URL, http: URL, or https: URL to import. Extensionless imports are parsed as YAML first, then TOML. Only imported rules are used; an imported config section is intentionally ignored. Known page URLs for GitHub files, GitHub Gists, GitLab files, GitLab snippets, Pastebin, Rentry, Hastebin, dpaste.org, Bitbucket files, and Codeberg/Gitea files are automatically converted to raw download URLs. Direct paste.rs, 0x0.st, and ttm.sh links are supported as regular URL imports."
                .to_string(),
        ),
    );
    import.insert(
        "examples".to_string(),
        json!([
            "rules/youtube.yaml",
            "https://github.com/jag-k/clipboard-transformer/blob/main/fixtures/youtube.yaml",
            "https://rentry.co/clipboard-transformer-rules"
        ]),
    );
}

fn enrich_rule_schema(schema: &mut Value) {
    if let Some(rule_schema) = schema
        .pointer_mut("/definitions/ConfigRuleSchema")
        .and_then(Value::as_object_mut)
    {
        if let Some(any_of) = rule_schema.remove("anyOf") {
            rule_schema.insert("oneOf".to_string(), any_of);
        }
        rule_schema.insert(
            "description".to_string(),
            Value::String(
                "A rule entry: import, a registered handler (regexp, url, url-cleanup, ruleset), a trusted native shell handler, or an unknown type ignored at runtime."
                    .to_string(),
            ),
        );
    }

    set_definition_title(schema, "ImportRuleSchema", "Import");
    set_definition_title(schema, "RegexpRuleSchema", "Regexp rule");
    set_definition_title(schema, "UrlRuleSchema", "URL rule");
    set_definition_title(schema, "UrlCleanupRuleSchema", "URL cleanup rule");
    set_definition_title(schema, "RulesetRuleSchema", "Ruleset rule");
    set_definition_title(schema, "ShellRuleSchema", "Native shell text rule");
    set_definition_title(schema, "ItemShellRuleSchema", "Native item shell rule");
    set_definition_title(schema, "UnknownRuleSchema", "Unknown rule type");

    for name in [
        "RegexpRuleSchema",
        "UrlRuleSchema",
        "UrlCleanupRuleSchema",
        "RulesetRuleSchema",
        "ShellRuleSchema",
        "ItemShellRuleSchema",
        "UnknownRuleSchema",
    ] {
        require_non_blank_rule_id(schema, name);
    }

    // serde(flatten) prevents schemars from emitting additionalProperties: false.
    for name in [
        "ImportRuleSchema",
        "RegexpRuleSchema",
        "UrlRuleSchema",
        "UrlCleanupRuleSchema",
        "RulesetRuleSchema",
        "ShellRuleSchema",
        "ItemShellRuleSchema",
    ] {
        set_additional_properties(schema, name, false);
    }

    for name in [
        "RegexpRuleSchema",
        "UrlRuleSchema",
        "UrlCleanupRuleSchema",
        "RulesetRuleSchema",
        "ShellRuleSchema",
        "ItemShellRuleSchema",
        "AppConfig",
    ] {
        require_app_mode_for_non_empty_apps(schema, name);
    }
    require_url_cleanup_matcher(schema);
    require_shell_script_source(schema, "ShellRuleSchema");
    require_shell_script_source(schema, "ItemShellRuleSchema");

    inline_rule_type_property(
        schema,
        "RegexpRuleSchema",
        "regexp",
        Some("regexp"),
        "Rule kind. Defaults to \"regexp\" when omitted; autocomplete may still offer this value explicitly.",
    );
    inline_rule_type_property(
        schema,
        "UrlRuleSchema",
        "url",
        None,
        "Rule kind for a structural URL-to-URL transform.",
    );
    inline_rule_type_property(
        schema,
        "UrlCleanupRuleSchema",
        "url-cleanup",
        None,
        "Rule kind for URL query cleanup.",
    );
    inline_rule_type_property(
        schema,
        "RulesetRuleSchema",
        "ruleset",
        None,
        "Rule kind for a nested ruleset.",
    );
    inline_rule_type_property(
        schema,
        "ShellRuleSchema",
        "shell",
        None,
        "Trusted native selected-text shell transform.",
    );
    inline_rule_type_property(
        schema,
        "ItemShellRuleSchema",
        "item-shell",
        None,
        "Trusted native full-item shell transform.",
    );

    if let Some(unknown) = schema
        .pointer_mut("/definitions/UnknownRuleSchema")
        .and_then(Value::as_object_mut)
    {
        unknown.insert("additionalProperties".to_string(), Value::Bool(true));
    }

    if let Some(kind) = schema
        .pointer_mut("/definitions/UnknownRuleSchema/properties/type")
        .and_then(Value::as_object_mut)
    {
        let mut reserved = REGISTERED_RULE_TYPES.to_vec();
        reserved.extend(["shell", "item-shell"]);
        kind.insert("not".to_string(), json!({ "enum": reserved }));
        kind.insert(
            "description".to_string(),
            Value::String(
                "Unrecognized rule kind. Must not overlap a built-in or native host rule. The runtime ignores these rules unless an installed plugin provides the type; extra fields are allowed."
                    .to_string(),
            ),
        );
    }

    // Single-value helper enums are inlined onto each variant's `type` for clearer IDE UX.
    remove_definition(schema, "RegexpRuleType");
    remove_definition(schema, "UrlRuleType");
    remove_definition(schema, "UrlCleanupRuleType");
    remove_definition(schema, "RulesetRuleType");
    remove_definition(schema, "ShellRuleType");
    remove_definition(schema, "ItemShellRuleType");
}

fn require_shell_script_source(schema: &mut Value, definition: &str) {
    let Some(rule) = schema
        .pointer_mut(&format!("/definitions/{definition}"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    rule.insert(
        "oneOf".to_string(),
        json!([
            {
                "required": ["run"],
                "not": { "required": ["script_path"] }
            },
            {
                "required": ["script_path"],
                "not": { "required": ["run"] }
            }
        ]),
    );
}

/// Inserts one `oneOf` variant per plugin rule type ahead of the unknown-type
/// fallback and excludes those types from the fallback so validation stays
/// unambiguous.
fn add_plugin_rule_variants(schema: &mut Value, plugins: &[PluginRuleSchemaContribution]) {
    if plugins.is_empty() {
        return;
    }

    for plugin in plugins {
        let definition_name = plugin_definition_name(&plugin.rule_type);
        let mut definition = json!({
            "title": format!("{} plugin rule", plugin.rule_type),
            "type": "object",
            "required": ["type", "id"],
            "additionalProperties": true,
            "properties": {
                "type": {
                    "type": "string",
                    "enum": [plugin.rule_type],
                    "description": plugin
                        .description
                        .clone()
                        .unwrap_or_else(|| "Plugin-provided rule type.".to_string()),
                },
                "id": {
                    "type": "string",
                    "pattern": r"\S",
                    "description": "Stable rule identifier. Required and used by notifications, undo, and disable state.",
                },
                "name": {
                    "type": ["string", "null"],
                    "description": "Optional short display label for notifications.",
                },
                "formats": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Ordered input format priority. Empty means the plugin's declared formats.",
                },
                "apps": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Source applications this rule filters on. Values match bundle id or app name.",
                },
                "app_mode": {
                    "description": "How to interpret apps: blacklist skips listed apps; whitelist only allows listed apps.",
                    "anyOf": [
                        { "$ref": "#/definitions/AppMode" },
                        { "type": "null" }
                    ]
                }
            }
        });
        // Plugin settings schemas with internal $refs would dangle once
        // embedded, so only self-contained schemas are attached.
        if let Some(settings_schema) = &plugin.settings_schema {
            if !serde_json::to_string(settings_schema)
                .unwrap_or_default()
                .contains("\"$ref\"")
            {
                let mut settings_schema = settings_schema.clone();
                if let Some(object) = settings_schema.as_object_mut() {
                    object.remove("$schema");
                    object.remove("additionalProperties");
                }
                definition
                    .as_object_mut()
                    .expect("plugin rule definition is an object")
                    .insert("allOf".to_string(), json!([settings_schema]));
            }
        }
        if let Some(definitions) = schema.get_mut("definitions").and_then(Value::as_object_mut) {
            definitions.insert(definition_name, definition);
        }
    }

    // Insert plugin variants before the unknown-type fallback.
    if let Some(one_of) = schema
        .pointer_mut("/definitions/ConfigRuleSchema/oneOf")
        .and_then(Value::as_array_mut)
    {
        let unknown_index = one_of
            .iter()
            .position(|variant| {
                variant.get("$ref").and_then(Value::as_str)
                    == Some("#/definitions/UnknownRuleSchema")
            })
            .unwrap_or(one_of.len());
        for (offset, plugin) in plugins.iter().enumerate() {
            let reference = format!(
                "#/definitions/{}",
                plugin_definition_name(&plugin.rule_type)
            );
            one_of.insert(unknown_index + offset, json!({ "$ref": reference }));
        }
    }

    // The unknown-type fallback must not overlap plugin-provided types.
    if let Some(kind) = schema
        .pointer_mut("/definitions/UnknownRuleSchema/properties/type/not/enum")
        .and_then(Value::as_array_mut)
    {
        kind.extend(
            plugins
                .iter()
                .map(|plugin| Value::String(plugin.rule_type.clone())),
        );
    }
}

fn plugin_definition_name(rule_type: &str) -> String {
    let sanitized: String = rule_type
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("PluginRuleSchema_{sanitized}")
}

fn set_definition_title(schema: &mut Value, name: &str, title: &str) {
    if let Some(definition) = schema
        .pointer_mut(&format!("/definitions/{name}"))
        .and_then(Value::as_object_mut)
    {
        definition.insert("title".to_string(), Value::String(title.to_string()));
    }
}

fn set_additional_properties(schema: &mut Value, name: &str, allowed: bool) {
    if let Some(definition) = schema
        .pointer_mut(&format!("/definitions/{name}"))
        .and_then(Value::as_object_mut)
    {
        definition.insert("additionalProperties".to_string(), Value::Bool(allowed));
    }
}

fn require_non_blank_rule_id(schema: &mut Value, definition: &str) {
    if let Some(id) = schema
        .pointer_mut(&format!("/definitions/{definition}/properties/id"))
        .and_then(Value::as_object_mut)
    {
        id.insert("pattern".to_string(), Value::String(r"\S".to_string()));
    }
}

fn require_app_mode_for_non_empty_apps(schema: &mut Value, definition: &str) {
    append_all_of(
        schema,
        definition,
        json!({
            "if": {
                "required": ["apps"],
                "properties": {
                    "apps": { "minItems": 1 }
                }
            },
            "then": {
                "required": ["app_mode"]
            }
        }),
    );
}

fn require_url_cleanup_matcher(schema: &mut Value) {
    let Some(definition) = schema
        .pointer_mut("/definitions/UrlCleanupRuleSchema")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    definition.insert(
        "anyOf".to_string(),
        json!([
            {
                "required": ["remove_query_params"],
                "properties": {
                    "remove_query_params": { "minItems": 1 }
                }
            },
            {
                "required": ["remove_query_prefixes"],
                "properties": {
                    "remove_query_prefixes": { "minItems": 1 }
                }
            },
            {
                "required": ["remove_query_param_patterns"],
                "properties": {
                    "remove_query_param_patterns": { "minItems": 1 }
                }
            }
        ]),
    );
}

fn append_all_of(schema: &mut Value, definition: &str, constraint: Value) {
    let Some(definition) = schema
        .pointer_mut(&format!("/definitions/{definition}"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    definition
        .entry("allOf")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("allOf generated as an array")
        .push(constraint);
}

fn inline_rule_type_property(
    schema: &mut Value,
    definition: &str,
    type_name: &str,
    default: Option<&str>,
    description: &str,
) {
    let Some(properties) = schema
        .pointer_mut(&format!("/definitions/{definition}/properties"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    let mut type_schema = serde_json::Map::new();
    type_schema.insert("type".to_string(), Value::String("string".to_string()));
    type_schema.insert("enum".to_string(), json!([type_name]));
    type_schema.insert(
        "description".to_string(),
        Value::String(description.to_string()),
    );
    if let Some(default) = default {
        type_schema.insert("default".to_string(), Value::String(default.to_string()));
    }
    properties.insert("type".to_string(), Value::Object(type_schema));
}

fn remove_definition(schema: &mut Value, name: &str) {
    if let Some(definitions) = schema.get_mut("definitions").and_then(Value::as_object_mut) {
        definitions.remove(name);
    }
}
