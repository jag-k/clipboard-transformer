//! Runtime-neutral Plugin API v1 types.
//!
//! These types define the product protocol independently of the Extism
//! adapter in [`super::runtime`]. Nothing in this module may depend on a
//! particular WASM runtime.

use anyhow::{bail, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use ct_clipboard::ClipboardSourceApp;

/// Guest exports every Plugin API v1 module must provide. Part of the protocol
/// contract, so it lives here rather than in whichever host runtime loads the
/// module.
pub const REQUIRED_EXPORTS: &[&str] = &["initialize", "compile_rule", "transform"];

/// Version of the host-plugin protocol. Incompatible manifests are rejected
/// at discovery time without executing plugin code.
pub const PLUGIN_API_VERSION: u32 = 1;

/// Name of the WASM custom section holding the embedded static manifest.
pub const MANIFEST_SECTION_NAME: &str = "clipboard-transformer/manifest";

/// Static identity and integration metadata embedded in the plugin module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PluginManifest {
    /// Stable namespaced plugin id, e.g. `dev.jag-k.gitlab`.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Plugin package version.
    pub version: String,
    /// Plugin API version this module implements.
    pub api_version: u32,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    /// Rule types provided by this plugin.
    pub rules: Vec<RuleDescriptor>,
    /// Optional JSON Schema for the plugin's opaque `settings` value.
    #[serde(default)]
    pub settings_schema: Option<serde_json::Value>,
    /// Capabilities the plugin asks for. Effective capabilities are the
    /// intersection of this list and the grants in the user config.
    #[serde(default)]
    pub capabilities: Vec<CapabilityRequest>,
    /// Structured setup instructions rendered by the host.
    #[serde(default)]
    pub instructions: Option<String>,
}

impl PluginManifest {
    /// Validates static invariants that discovery relies on.
    pub fn validate(&self) -> Result<()> {
        validate_plugin_id(&self.id)?;
        if self.api_version != PLUGIN_API_VERSION {
            bail!(
                "plugin {:?} targets unsupported plugin API version {} (host supports {})",
                self.id,
                self.api_version,
                PLUGIN_API_VERSION
            );
        }
        if self.name.trim().is_empty() {
            bail!("plugin {:?} has an empty display name", self.id);
        }
        if self.rules.is_empty() {
            bail!("plugin {:?} declares no rule types", self.id);
        }
        let mut seen = std::collections::BTreeSet::new();
        for rule in &self.rules {
            validate_rule_type_name(&self.id, &rule.rule_type)?;
            if !seen.insert(rule.rule_type.as_str()) {
                bail!(
                    "plugin {:?} declares duplicate rule type {:?}",
                    self.id,
                    rule.rule_type
                );
            }
        }
        Ok(())
    }

    /// Full namespaced rule type ids contributed by this plugin.
    pub fn rule_type_ids(&self) -> impl Iterator<Item = String> + '_ {
        self.rules
            .iter()
            .map(|rule| namespaced_rule_type(&self.id, &rule.rule_type))
    }

    pub fn requests_capability(&self, kind: CapabilityKind) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability.kind() == kind)
    }
}

/// Builds the config-facing rule type id: `<plugin-id>/<rule-type>`.
pub fn namespaced_rule_type(plugin_id: &str, rule_type: &str) -> String {
    format!("{plugin_id}/{rule_type}")
}

/// Splits a namespaced rule type back into plugin id and local rule type.
pub fn split_rule_type(namespaced: &str) -> Option<(&str, &str)> {
    namespaced.split_once('/')
}

fn validate_plugin_id(id: &str) -> Result<()> {
    if id.trim().is_empty() {
        bail!("plugin id cannot be empty");
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        bail!("plugin id {id:?} may only contain ASCII letters, digits, '.', '_', and '-'");
    }
    Ok(())
}

fn validate_rule_type_name(plugin_id: &str, rule_type: &str) -> Result<()> {
    if rule_type.trim().is_empty() {
        bail!("plugin {plugin_id:?} declares an empty rule type name");
    }
    if !rule_type
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        bail!(
            "plugin {plugin_id:?} rule type {rule_type:?} may only contain \
             ASCII letters, digits, '.', '_', and '-'"
        );
    }
    Ok(())
}

/// One rule type provided by a plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RuleDescriptor {
    /// Local rule type name; the config-facing id is `<plugin-id>/<type>`.
    #[serde(rename = "type")]
    pub rule_type: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Optional JSON Schema for this rule's settings.
    #[serde(default)]
    pub settings_schema: Option<serde_json::Value>,
    /// Example rule configurations rendered by the host.
    #[serde(default)]
    pub examples: Vec<serde_json::Value>,
    /// Accepted clipboard formats in priority order. Empty means plain text.
    #[serde(default)]
    pub formats: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CapabilityKind {
    Http,
    EnvExpansion,
}

impl CapabilityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::EnvExpansion => "env-expansion",
        }
    }
}

/// A capability requested in the manifest with a human-readable reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CapabilityRequest {
    Http {
        #[serde(default)]
        reason: Option<String>,
    },
    EnvExpansion {
        #[serde(default)]
        reason: Option<String>,
    },
}

impl CapabilityRequest {
    pub fn kind(&self) -> CapabilityKind {
        match self {
            Self::Http { .. } => CapabilityKind::Http,
            Self::EnvExpansion { .. } => CapabilityKind::EnvExpansion,
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Http { reason } | Self::EnvExpansion { reason } => reason.as_deref(),
        }
    }
}

/// Capabilities the host actually granted: `requested ∩ configured`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GrantedCapabilities {
    /// Whether environment expansion ran over the plugin settings.
    #[serde(default)]
    pub env_expansion: bool,
}

/// `initialize` export input.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InitializeRequest {
    pub api_version: u32,
    /// Resolved opaque plugin settings (after any granted env expansion).
    #[schemars(with = "Option<std::collections::BTreeMap<String, serde_json::Value>>")]
    pub settings: serde_json::Value,
    pub granted_capabilities: GrantedCapabilities,
}

/// `initialize` export output.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct InitializeResponse {
    pub status: PluginHealth,
    /// Available rule type names (local, un-namespaced). `None` means every
    /// declared rule type is available.
    #[serde(default)]
    pub available_rules: Option<Vec<String>>,
    #[serde(default)]
    pub issues: Vec<PluginIssue>,
}

/// Plugin-reported functional health, distinct from host-owned load and
/// runtime failures (see [`super::PluginState`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PluginHealth {
    Operational,
    Degraded,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum IssueSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttentionLevel {
    #[default]
    None,
    Informational,
    ActionRequired,
}

/// A structured plugin or host issue surfaced to CLI, tray, and notifications.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PluginIssue {
    /// Stable machine-readable code.
    pub code: String,
    pub severity: IssueSeverity,
    pub summary: String,
    #[serde(default)]
    pub details: Option<String>,
    /// Path inside the plugin settings this issue refers to.
    #[serde(default)]
    pub setting_path: Option<String>,
    /// Affected local rule type names.
    #[serde(default)]
    pub rule_types: Vec<String>,
    /// Whether user action is needed: `none`, `informational`, or
    /// `action-required`.
    #[serde(default)]
    #[schemars(with = "String")]
    pub attention: AttentionLevel,
}

impl PluginIssue {
    pub fn host(code: &str, severity: IssueSeverity, summary: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            severity,
            summary: summary.into(),
            details: None,
            setting_path: None,
            rule_types: Vec::new(),
            attention: AttentionLevel::ActionRequired,
        }
    }
}

/// `compile_rule` export input.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompileRuleRequest {
    /// Local rule type name.
    pub rule_type: String,
    /// Rule settings: every configured key except the shared rule fields.
    #[schemars(with = "std::collections::BTreeMap<String, serde_json::Value>")]
    pub settings: serde_json::Value,
}

/// `compile_rule` export output.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "result", rename_all = "kebab-case")]
pub enum CompileRuleResponse {
    /// Opaque compiled rule value passed back verbatim on every transform.
    Ok {
        rule: serde_json::Value,
    },
    Error {
        message: String,
    },
}

/// `transform` export input for text-transform rules.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TransformRequest {
    /// Local rule type name.
    pub rule_type: String,
    /// Opaque compiled rule value returned by `compile_rule`.
    #[schemars(with = "std::collections::BTreeMap<String, serde_json::Value>")]
    pub rule: serde_json::Value,
    /// Selected clipboard format id.
    pub format: String,
    /// Selected UTF-8 representation.
    pub value: String,
    /// Best-effort source application metadata.
    #[serde(default)]
    pub source_app: Option<ClipboardSourceApp>,
}

/// `transform` export output.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum TransformResponse {
    /// Preserve the complete input item.
    NoMatch,
    /// Replace the item with plain text, like built-in text transforms.
    Replace {
        text: String,
        #[serde(default)]
        message: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_json() -> serde_json::Value {
        serde_json::json!({
            "id": "dev.jag-k.gitlab",
            "name": "GitLab Links",
            "version": "0.1.0",
            "api_version": 1,
            "rules": [{"type": "human-readable-link"}],
            "capabilities": [{"kind": "http", "reason": "fetch titles"}],
        })
    }

    #[test]
    fn manifest_parses_and_validates() {
        let manifest: PluginManifest = serde_json::from_value(manifest_json()).unwrap();
        manifest.validate().unwrap();
        assert_eq!(
            manifest.rule_type_ids().collect::<Vec<_>>(),
            ["dev.jag-k.gitlab/human-readable-link"]
        );
        assert!(manifest.requests_capability(CapabilityKind::Http));
        assert!(!manifest.requests_capability(CapabilityKind::EnvExpansion));
    }

    #[test]
    fn manifest_ignores_unknown_fields_without_retaining_them() {
        let mut value = manifest_json();
        value["$schema"] = serde_json::json!(
            "https://raw.githubusercontent.com/jag-k/clipboard-transformer/main/plugins/manifest.schema.json"
        );
        value["future_metadata"] = serde_json::json!({"producer": "example"});
        value["rules"][0]["future_rule_metadata"] = serde_json::json!(true);
        value["capabilities"][0]["future_capability_metadata"] = serde_json::json!(true);

        let manifest: PluginManifest = serde_json::from_value(value).unwrap();
        manifest.validate().unwrap();
        let serialized = serde_json::to_value(manifest).unwrap();
        assert!(serialized.get("$schema").is_none());
        assert!(serialized.get("future_metadata").is_none());
        assert!(serialized["rules"][0].get("future_rule_metadata").is_none());
        assert!(serialized["capabilities"][0]
            .get("future_capability_metadata")
            .is_none());
    }

    #[test]
    fn manifest_rejects_wrong_api_version() {
        let mut value = manifest_json();
        value["api_version"] = serde_json::json!(2);
        let manifest: PluginManifest = serde_json::from_value(value).unwrap();
        let error = manifest.validate().unwrap_err().to_string();
        assert!(error.contains("unsupported plugin API version"), "{error}");
    }

    #[test]
    fn manifest_rejects_slash_in_ids() {
        let mut value = manifest_json();
        value["id"] = serde_json::json!("dev/evil");
        let manifest: PluginManifest = serde_json::from_value(value).unwrap();
        assert!(manifest.validate().is_err());

        let mut value = manifest_json();
        value["rules"][0]["type"] = serde_json::json!("a/b");
        let manifest: PluginManifest = serde_json::from_value(value).unwrap();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn manifest_rejects_duplicate_rule_types() {
        let mut value = manifest_json();
        value["rules"] = serde_json::json!([
            {"type": "human-readable-link"},
            {"type": "human-readable-link"},
        ]);
        let manifest: PluginManifest = serde_json::from_value(value).unwrap();
        let error = manifest.validate().unwrap_err().to_string();
        assert!(error.contains("duplicate rule type"), "{error}");
    }

    #[test]
    fn transform_response_parses_both_actions() {
        let no_match: TransformResponse =
            serde_json::from_str(r#"{"action": "no-match"}"#).unwrap();
        assert!(matches!(no_match, TransformResponse::NoMatch));

        let replace: TransformResponse =
            serde_json::from_str(r#"{"action": "replace", "text": "hi"}"#).unwrap();
        match replace {
            TransformResponse::Replace { text, message } => {
                assert_eq!(text, "hi");
                assert_eq!(message, None);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn transform_request_remains_a_narrow_text_transform_contract() {
        let request = TransformRequest {
            rule_type: "example".into(),
            rule: serde_json::json!({}),
            format: "text".into(),
            value: "cat".into(),
            source_app: Some(ct_clipboard::ClipboardSourceApp::new(
                Some("com.example.Editor".into()),
                Some("Editor".into()),
            )),
        };

        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["format"], "text");
        assert_eq!(json["value"], "cat");
        assert_eq!(json["source_app"]["bundle_id"], "com.example.Editor");
        assert!(json.get("item").is_none());
    }

    #[test]
    fn split_rule_type_returns_plugin_and_local_parts() {
        assert_eq!(
            split_rule_type("dev.jag-k.gitlab/human-readable-link"),
            Some(("dev.jag-k.gitlab", "human-readable-link"))
        );
        assert_eq!(split_rule_type("regexp"), None);
    }
}
