mod provider;
mod regexp;
mod ruleset;
mod url;

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeSet;

#[doc(hidden)]
pub use ct_clipboard::ClipboardFingerprint;
pub use ct_clipboard::{
    decode_latin1, decode_mime_text, normalize_format, ClipboardFormat, ClipboardItem,
    ClipboardPlatform, ClipboardSourceApp, NativeFormatFlag, NativeRepresentation, SemanticValue,
    SemanticViews,
};
use provider::RuleProviderRegistry;

#[derive(Debug, Clone, Default, PartialEq, Serialize, JsonSchema)]
pub struct RawRule {
    /// Rule kind. Defaults to "regexp".
    #[serde(default)]
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Stable rule identifier. Required and used by notifications, undo, and disable state.
    pub id: String,
    /// Optional short display label for notifications.
    #[serde(default)]
    pub name: Option<String>,
    /// Regex pattern for regexp rules.
    #[serde(default)]
    pub from: Option<String>,
    /// Replacement template for regexp rules.
    #[serde(default)]
    pub to: Option<String>,
    /// Optional regexp flags: i (case-insensitive), m (multi-line), s (dot matches newline), U (swap greed), x (ignore whitespace), or u (Unicode).
    #[serde(default)]
    pub flags: Option<String>,
    /// Optional notification body template. Regex captures like $1 and $name are expanded.
    #[serde(default)]
    pub message: Option<String>,
    /// Ordered input format priority for text transforms; activation filter for item transforms. Empty means text. Common aliases are text/plain-text, url, html, rtf, and file/file-url; native format ids are accepted directly.
    #[serde(default)]
    pub formats: Vec<String>,
    /// Nested ruleset application mode.
    #[serde(default)]
    pub mode: Option<RulesetMode>,
    /// Nested rules for ruleset rules.
    #[serde(default)]
    pub rules: Vec<RawRule>,
    /// Structural URL transformation for `url` rules.
    #[serde(default, rename = "transform")]
    pub url_transform: Option<UrlTransform>,
    /// URL hosts this rule applies to. Empty means all hosts.
    #[serde(default)]
    pub hosts: Vec<String>,
    /// Exact query parameter names to remove from parsed URLs.
    #[serde(default)]
    pub remove_query_params: Vec<String>,
    /// Query parameter prefixes to remove from parsed URLs.
    #[serde(default)]
    pub remove_query_prefixes: Vec<String>,
    /// Regex patterns for query parameter names to remove from parsed URLs.
    #[serde(default)]
    pub remove_query_param_patterns: Vec<String>,
    /// Source applications this rule filters on. Values match bundle id or app name.
    #[serde(default)]
    pub apps: Vec<String>,
    /// How to interpret apps: blacklist skips listed apps; whitelist only allows listed apps.
    #[serde(default)]
    pub app_mode: Option<AppMode>,
    /// Rule-type-specific settings retained for external (plugin) rule types.
    /// Serialized inline so imported plugin rules survive round trips.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub plugin_settings: Option<serde_json::Map<String, serde_json::Value>>,
    /// Deserialization failure retained so config loading can skip only this rule.
    #[doc(hidden)]
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AppMode {
    Blacklist,
    Whitelist,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RulesetMode {
    #[default]
    AllMatching,
    WhileMatching,
    All,
    First,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum UrlTransform {
    /// Remove selected raw query segments while preserving every untouched
    /// segment's original encoding.
    RemoveQueryParams {
        #[serde(default)]
        names: Vec<String>,
        #[serde(default)]
        prefixes: Vec<String>,
        #[serde(default)]
        patterns: Vec<String>,
    },
    /// Remove complete structural URL components.
    RemoveComponents { components: Vec<UrlComponent> },
    /// Replace the URL host while retaining the remaining components. When
    /// `from` is set, the current host must match it exactly.
    RewriteHost {
        #[serde(default)]
        from: Option<String>,
        to: String,
    },
    /// Replace the URL scheme while retaining the remaining components. When
    /// `from` is set, the current scheme must match it exactly.
    RewriteScheme {
        #[serde(default)]
        from: Option<String>,
        to: String,
    },
    /// Replace every query parameter with this name and append one encoded
    /// name/value pair without re-encoding untouched parameters.
    SetQueryParam { name: String, value: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum UrlComponent {
    Fragment,
    Query,
    Credentials,
    Port,
    Path,
}

impl<'de> Deserialize<'de> for RawRule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let kind = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("regexp");

        if !is_registered_rule_type(kind) {
            // External (plugin) rule types keep their shared fields plus every
            // remaining key as opaque settings; whether the type is usable is
            // decided later against the discovered plugin set.
            return Ok(match ExternalRawRule::deserialize(value.clone()) {
                Ok(rule) => rule.into(),
                Err(error) => Self {
                    kind: Some(kind.to_string()),
                    id: value
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: value
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    validation_error: Some(error.to_string()),
                    ..Self::default()
                },
            });
        }

        match KnownRawRule::deserialize(value.clone()) {
            Ok(rule) => Ok(rule.into()),
            Err(error) => Ok(Self {
                kind: value
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                id: value
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: value
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                validation_error: Some(error.to_string()),
                ..Self::default()
            }),
        }
    }
}

#[derive(Deserialize)]
struct KnownRawRule {
    #[serde(default, rename = "type")]
    kind: Option<String>,
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    flags: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    formats: Vec<String>,
    #[serde(default)]
    mode: Option<RulesetMode>,
    #[serde(default)]
    rules: Vec<RawRule>,
    #[serde(default, rename = "transform")]
    url_transform: Option<UrlTransform>,
    #[serde(default)]
    hosts: Vec<String>,
    #[serde(default)]
    remove_query_params: Vec<String>,
    #[serde(default)]
    remove_query_prefixes: Vec<String>,
    #[serde(default)]
    remove_query_param_patterns: Vec<String>,
    #[serde(default)]
    apps: Vec<String>,
    #[serde(default)]
    app_mode: Option<AppMode>,
}

impl From<KnownRawRule> for RawRule {
    fn from(rule: KnownRawRule) -> Self {
        Self {
            kind: rule.kind,
            id: rule.id,
            name: rule.name,
            from: rule.from,
            to: rule.to,
            flags: rule.flags,
            message: rule.message,
            formats: rule.formats,
            mode: rule.mode,
            rules: rule.rules,
            url_transform: rule.url_transform,
            hosts: rule.hosts,
            remove_query_params: rule.remove_query_params,
            remove_query_prefixes: rule.remove_query_prefixes,
            remove_query_param_patterns: rule.remove_query_param_patterns,
            apps: rule.apps,
            app_mode: rule.app_mode,
            plugin_settings: None,
            validation_error: None,
        }
    }
}

/// Shared rule fields plus opaque settings, used for plugin rule types.
#[derive(Deserialize)]
struct ExternalRawRule {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    formats: Vec<String>,
    #[serde(default)]
    apps: Vec<String>,
    #[serde(default)]
    app_mode: Option<AppMode>,
    #[serde(flatten)]
    settings: serde_json::Map<String, serde_json::Value>,
}

impl From<ExternalRawRule> for RawRule {
    fn from(rule: ExternalRawRule) -> Self {
        Self {
            kind: Some(rule.kind),
            id: rule.id,
            name: rule.name,
            formats: rule.formats,
            apps: rule.apps,
            app_mode: rule.app_mode,
            plugin_settings: Some(rule.settings),
            ..Self::default()
        }
    }
}

/// Built-in rule `type` values. Keep in sync with config schema variants.
/// Plugin rule types are namespaced (`<plugin-id>/<type>`) and registered
/// dynamically through [`ExternalRuleProvider`].
#[doc(hidden)]
pub const REGISTERED_RULE_TYPES: &[&str] = &["regexp", "url", "url-cleanup", "ruleset"];

#[doc(hidden)]
pub fn is_registered_rule_type(kind: &str) -> bool {
    REGISTERED_RULE_TYPES.contains(&kind)
}

/// Output of an external text transform, mirroring built-in text rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalTextOutput {
    pub text: String,
    pub message: Option<String>,
}

/// Output of an external full-item transform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalItemOutput {
    pub item: ClipboardItem,
    pub message: Option<String>,
}

/// One compiled external rule instance. Implementations run on the rule
/// engine worker thread and must contain a failed transform rather than
/// panicking the host.
pub trait ExternalTextTransform: Send {
    fn transform(
        &mut self,
        format: &str,
        value: &str,
        source_app: Option<&ClipboardSourceApp>,
    ) -> Result<Option<ExternalTextOutput>>;
}

/// One compiled external full-item transform.
pub trait ExternalItemTransform: Send {
    fn transform(&mut self, item: &ClipboardItem) -> Result<Option<ExternalItemOutput>>;
}

/// The transformation contract selected by one external rule provider.
pub enum ExternalTransform {
    Text(Box<dyn ExternalTextTransform>),
    Item(Box<dyn ExternalItemTransform>),
}

/// A dynamically registered rule type. The engine owns filtering, format
/// selection, and pipeline order; providers own their opaque settings and
/// either selected-text or complete-item transformation.
pub trait ExternalRuleProvider: Send + Sync {
    /// Full namespaced rule type id, e.g. `dev.jag-k.gitlab/human-readable-link`.
    fn kind(&self) -> &str;

    /// Accepted formats, in priority order, used when a rule sets none.
    fn default_formats(&self) -> &[String];

    /// Compiles one configured rule's opaque settings into a transform.
    fn compile(&self, rule_id: &str, settings: &serde_json::Value) -> Result<ExternalTransform>;
}

/// A configured external rule the engine dropped because its provider
/// rejected the settings. Reported instead of failing the whole config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedExternalRule {
    pub id: String,
    pub kind: String,
    pub reason: String,
}

pub struct RuleEngine {
    ruleset: provider::CompiledRuleset,
    required_formats: BTreeSet<ClipboardFormat>,
}

impl std::fmt::Debug for RuleEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuleEngine")
            .field("rule_count", &self.rule_count())
            .field("required_formats", &self.required_formats)
            .finish()
    }
}

impl RuleEngine {
    pub fn compile(raw_rules: Vec<RawRule>) -> Result<Self> {
        let providers = RuleProviderRegistry::builtins();
        let rules = providers.compile_rules(raw_rules)?;
        Ok(Self::from_rules(rules))
    }

    /// Compiles rules with additional dynamically registered (plugin) rule
    /// types. External rules whose provider rejects their settings are
    /// skipped and reported instead of failing the configuration.
    pub fn compile_with_external(
        raw_rules: Vec<RawRule>,
        external: &[std::sync::Arc<dyn ExternalRuleProvider>],
    ) -> Result<(Self, Vec<SkippedExternalRule>)> {
        let providers = RuleProviderRegistry::with_external(external);
        let rules = providers.compile_rules(raw_rules)?;
        let skipped = providers.take_skipped_external();
        Ok((Self::from_rules(rules), skipped))
    }

    pub fn apply(&mut self, input: &ClipboardItem) -> Option<TransformResult> {
        self.try_apply(input)
            .expect("rule transform failed in infallible compatibility API")
    }

    pub fn try_apply(&mut self, input: &ClipboardItem) -> Result<Option<TransformResult>> {
        self.try_apply_excluding(input, &BTreeSet::new())
    }

    pub fn apply_excluding(
        &mut self,
        input: &ClipboardItem,
        disabled_rule_ids: &BTreeSet<String>,
    ) -> Option<TransformResult> {
        self.try_apply_excluding(input, disabled_rule_ids)
            .expect("rule transform failed in infallible compatibility API")
    }

    pub fn try_apply_excluding(
        &mut self,
        input: &ClipboardItem,
        disabled_rule_ids: &BTreeSet<String>,
    ) -> Result<Option<TransformResult>> {
        Ok(self
            .ruleset
            .apply(input, disabled_rule_ids)?
            .map(|outcome| TransformResult {
                before: input.clone(),
                after: outcome.content,
                applied_rules: outcome.applied_rules,
                message: outcome.message,
            }))
    }

    pub fn try_apply_owned_excluding(
        &mut self,
        input: ClipboardItem,
        disabled_rule_ids: &BTreeSet<String>,
    ) -> Result<Option<TransformResult>> {
        let outcome = self.ruleset.apply(&input, disabled_rule_ids)?;
        Ok(outcome.map(|outcome| TransformResult {
            before: input,
            after: outcome.content,
            applied_rules: outcome.applied_rules,
            message: outcome.message,
        }))
    }

    pub fn rule_count(&self) -> usize {
        self.ruleset.rule_count()
    }

    #[cfg(test)]
    fn compiled_node_count(&self) -> usize {
        self.ruleset.compiled_node_count()
    }

    #[cfg(test)]
    fn compact_chain_depth(&self) -> usize {
        self.ruleset.compact_chain_depth()
    }

    pub fn required_formats(&self) -> &BTreeSet<ClipboardFormat> {
        &self.required_formats
    }

    fn from_rules(ruleset: provider::CompiledRuleset) -> Self {
        let mut required_formats = BTreeSet::new();
        ruleset.collect_formats(&mut required_formats);
        Self {
            ruleset,
            required_formats,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformResult {
    pub before: ClipboardItem,
    pub after: ClipboardItem,
    pub applied_rules: Vec<AppliedRule>,
    pub message: Option<String>,
}

impl TransformResult {
    pub fn applied_rule_ids(&self) -> impl Iterator<Item = &str> {
        self.applied_rules.iter().map(|rule| rule.id.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedRule {
    pub id: String,
    pub name: Option<String>,
}

impl AppliedRule {
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

#[derive(Debug, Clone)]
pub struct AppMatcher {
    apps: Vec<String>,
    mode: Option<AppMode>,
}

impl AppMatcher {
    pub fn compile(apps: Vec<String>, mode: Option<AppMode>) -> Result<Self> {
        if !apps.is_empty() && mode.is_none() {
            bail!("app filter requires app_mode");
        }
        Ok(Self { apps, mode })
    }

    pub fn allows_app(&self, app: Option<&ClipboardSourceApp>) -> bool {
        let Some(mode) = self.mode else {
            return true;
        };

        let matched = app.is_some_and(|app| app.matches_any(&self.apps));
        match mode {
            AppMode::Blacklist => !matched,
            AppMode::Whitelist => matched,
        }
    }

    pub(crate) fn allows(&self, input: &ClipboardItem) -> bool {
        self.allows_app(input.source_app())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuleOutcome {
    pub(crate) content: ClipboardItem,
    pub(crate) applied_rules: Vec<AppliedRule>,
    pub(crate) message: Option<String>,
}

pub(crate) fn required_rule_id(id: String) -> Result<String> {
    if id.trim().is_empty() {
        bail!("rule id cannot be empty");
    }
    Ok(id)
}

pub(crate) fn normalize_formats(formats: Vec<String>) -> Result<Vec<ClipboardFormat>> {
    let formats = if formats.is_empty() {
        vec!["text".to_string()]
    } else {
        formats
    };

    formats
        .into_iter()
        .map(|format| {
            normalize_format(&format).with_context(|| format!("invalid format {format:?}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile_linear(raw_rules: Vec<RawRule>) -> RuleEngine {
        let providers = RuleProviderRegistry::builtins();
        RuleEngine::from_rules(providers.compile_rules_linear(raw_rules).unwrap())
    }

    fn apply_sequence(
        raw_rules: Vec<RawRule>,
        mode: RulesetMode,
        compact: bool,
        value: &str,
        disabled: &BTreeSet<String>,
    ) -> Option<provider::ItemTransformOutput> {
        let providers = RuleProviderRegistry::builtins();
        let mut ruleset = if compact {
            providers.compile_rules_for_mode(raw_rules, mode).unwrap()
        } else {
            providers
                .compile_rules_linear_for_mode(raw_rules, mode)
                .unwrap()
        };
        ruleset
            .apply(&ClipboardItem::from_text(value), disabled)
            .unwrap()
    }

    fn cleanup(id: &str, param: &str, message: Option<&str>) -> RawRule {
        RawRule {
            kind: Some("url-cleanup".into()),
            id: id.into(),
            message: message.map(str::to_string),
            remove_query_params: vec![param.into()],
            ..RawRule::default()
        }
    }

    fn url_rule(id: &str, transform: UrlTransform) -> RawRule {
        RawRule {
            kind: Some("url".into()),
            id: id.into(),
            url_transform: Some(transform),
            ..RawRule::default()
        }
    }

    #[test]
    fn adjacent_url_cleanup_rules_are_compact_and_match_linear_execution() {
        let raw_rules = vec![
            cleanup("drop-a", "a", Some("first message")),
            cleanup("drop-b", "b", Some("second distinct message")),
            RawRule {
                id: "barrier".into(),
                from: Some("example".into()),
                to: Some("sample".into()),
                message: Some("barrier message".into()),
                ..RawRule::default()
            },
            cleanup("drop-c", "c", Some("third distinct message")),
            cleanup("drop-d", "d", None),
        ];
        let mut compact = RuleEngine::compile(raw_rules.clone()).unwrap();
        let mut linear = compile_linear(raw_rules);

        assert_eq!(compact.rule_count(), 5);
        assert_eq!(compact.compiled_node_count(), 3);
        for disabled in [
            BTreeSet::new(),
            BTreeSet::from(["drop-b".to_string()]),
            BTreeSet::from(["barrier".to_string(), "drop-d".to_string()]),
        ] {
            for value in [
                "https://example.com/path?a=1&b=2&c=3&d=4",
                "https://other.test/path?a=1&d=4",
                "plain text",
            ] {
                assert_eq!(
                    compact
                        .try_apply_excluding(&ClipboardItem::from_text(value), &disabled)
                        .unwrap(),
                    linear
                        .try_apply_excluding(&ClipboardItem::from_text(value), &disabled)
                        .unwrap(),
                );
            }
        }
    }

    #[test]
    fn every_ruleset_mode_compacts_adjacent_url_cleanup_rules() {
        for mode in [
            RulesetMode::WhileMatching,
            RulesetMode::All,
            RulesetMode::First,
        ] {
            let providers = RuleProviderRegistry::builtins();
            let rules = providers
                .compile_rules_for_mode(
                    vec![cleanup("drop-a", "a", None), cleanup("drop-b", "b", None)],
                    mode,
                )
                .unwrap();
            assert_eq!(rules.compiled_node_count(), 1, "{mode:?}");
        }
    }

    #[test]
    fn compact_url_batches_match_linear_execution_in_every_ruleset_mode() {
        let raw_rules = vec![
            cleanup("drop-a", "a", Some("removed a")),
            cleanup("drop-b", "b", Some("removed b")),
            RawRule {
                id: "barrier".into(),
                from: Some("example".into()),
                to: Some("sample".into()),
                message: Some("barrier".into()),
                ..RawRule::default()
            },
        ];
        let values = [
            "https://example.com/?a=1&b=2",
            "https://example.com/?a=1",
            "https://example.com/?b=2",
            "https://example.com/?c=3",
            "plain example text",
        ];
        let disabled_sets = [
            BTreeSet::new(),
            BTreeSet::from(["drop-a".to_string()]),
            BTreeSet::from(["drop-b".to_string(), "barrier".to_string()]),
        ];

        for mode in [
            RulesetMode::AllMatching,
            RulesetMode::WhileMatching,
            RulesetMode::All,
            RulesetMode::First,
        ] {
            for value in values {
                for disabled in &disabled_sets {
                    assert_eq!(
                        apply_sequence(raw_rules.clone(), mode, true, value, disabled),
                        apply_sequence(raw_rules.clone(), mode, false, value, disabled),
                        "mode={mode:?} value={value:?} disabled={disabled:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn mixed_url_and_legacy_cleanup_rules_share_one_batch() {
        let providers = RuleProviderRegistry::builtins();
        let rules = providers
            .compile_rules_for_mode(
                vec![
                    cleanup("legacy", "tracking", None),
                    url_rule(
                        "fragment",
                        UrlTransform::RemoveComponents {
                            components: vec![UrlComponent::Fragment],
                        },
                    ),
                    url_rule(
                        "host",
                        UrlTransform::RewriteHost {
                            from: None,
                            to: "example.com".into(),
                        },
                    ),
                ],
                RulesetMode::AllMatching,
            )
            .unwrap();
        assert_eq!(rules.compiled_node_count(), 1);
    }

    #[test]
    fn structural_url_transforms_apply_in_order_without_reencoding_kept_query() {
        let mut engine = RuleEngine::compile(vec![
            url_rule(
                "remove",
                UrlTransform::RemoveQueryParams {
                    names: vec!["tracking".into()],
                    prefixes: Vec::new(),
                    patterns: Vec::new(),
                },
            ),
            url_rule(
                "set",
                UrlTransform::SetQueryParam {
                    name: "view".into(),
                    value: "a b".into(),
                },
            ),
            url_rule(
                "components",
                UrlTransform::RemoveComponents {
                    components: vec![UrlComponent::Fragment, UrlComponent::Credentials],
                },
            ),
            url_rule(
                "host",
                UrlTransform::RewriteHost {
                    from: None,
                    to: "example.com".into(),
                },
            ),
            url_rule(
                "scheme",
                UrlTransform::RewriteScheme {
                    from: None,
                    to: "https".into(),
                },
            ),
        ])
        .unwrap();

        let result = engine
            .apply(&ClipboardItem::from_text(
                "http://user:pass@www.example.com/a?keep=%20&tracking=1#part",
            ))
            .unwrap();

        assert_eq!(
            result.after.text(),
            Some("https://example.com/a?keep=%20&view=a+b")
        );
        assert_eq!(result.applied_rules.len(), 5);
    }

    #[test]
    fn legacy_url_cleanup_and_url_remove_query_params_are_equivalent() {
        let legacy = cleanup("legacy", "tracking", Some("cleaned"));
        let modern = url_rule(
            "modern",
            UrlTransform::RemoveQueryParams {
                names: vec!["tracking".into()],
                prefixes: Vec::new(),
                patterns: Vec::new(),
            },
        );
        let value =
            ClipboardItem::from_text("https://example.com/a?keep=%20&tracking=1&flag#fragment");
        let mut legacy_engine = RuleEngine::compile(vec![legacy]).unwrap();
        let mut modern_engine = RuleEngine::compile(vec![modern]).unwrap();

        assert_eq!(
            legacy_engine.apply(&value).unwrap().after,
            modern_engine.apply(&value).unwrap().after
        );
    }

    #[test]
    fn mixed_structural_url_batches_match_linear_execution_in_every_mode() {
        let raw_rules = vec![
            cleanup("legacy", "tracking", None),
            url_rule(
                "fragment",
                UrlTransform::RemoveComponents {
                    components: vec![UrlComponent::Fragment],
                },
            ),
            url_rule(
                "host",
                UrlTransform::RewriteHost {
                    from: None,
                    to: "example.com".into(),
                },
            ),
        ];
        let values = [
            "https://www.example.com/a?tracking=1#part",
            "https://www.example.com/a?keep=1#part",
            "https://example.com/a?tracking=1",
            "plain text",
        ];

        for mode in [
            RulesetMode::AllMatching,
            RulesetMode::WhileMatching,
            RulesetMode::All,
            RulesetMode::First,
        ] {
            for value in values {
                assert_eq!(
                    apply_sequence(raw_rules.clone(), mode, true, value, &BTreeSet::new()),
                    apply_sequence(raw_rules.clone(), mode, false, value, &BTreeSet::new()),
                    "mode={mode:?} value={value:?}"
                );
            }
        }
    }

    #[test]
    fn url_rules_reject_incomplete_or_unsafe_transforms() {
        let missing = RawRule {
            kind: Some("url".into()),
            id: "missing".into(),
            ..RawRule::default()
        };
        assert!(RuleEngine::compile(vec![missing]).is_err());

        let empty_remove = url_rule(
            "empty-remove",
            UrlTransform::RemoveQueryParams {
                names: Vec::new(),
                prefixes: Vec::new(),
                patterns: Vec::new(),
            },
        );
        assert!(RuleEngine::compile(vec![empty_remove]).is_err());

        let unsafe_scheme = url_rule(
            "unsafe-scheme",
            UrlTransform::RewriteScheme {
                from: None,
                to: "javascript".into(),
            },
        );
        assert!(RuleEngine::compile(vec![unsafe_scheme]).is_err());
    }

    #[test]
    fn url_rewrite_from_guards_and_removable_components_are_explicit() {
        let mut guarded = RuleEngine::compile(vec![
            url_rule(
                "host",
                UrlTransform::RewriteHost {
                    from: Some("old.example.com".into()),
                    to: "new.example.com".into(),
                },
            ),
            url_rule(
                "scheme",
                UrlTransform::RewriteScheme {
                    from: Some("http".into()),
                    to: "https".into(),
                },
            ),
        ])
        .unwrap();
        assert!(guarded
            .apply(&ClipboardItem::from_text("http://other.example.com/a"))
            .is_some());
        assert_eq!(
            guarded
                .apply(&ClipboardItem::from_text("http://old.example.com/a"))
                .unwrap()
                .after
                .text(),
            Some("https://new.example.com/a")
        );

        let mut remove = RuleEngine::compile(vec![url_rule(
            "remove",
            UrlTransform::RemoveComponents {
                components: vec![
                    UrlComponent::Path,
                    UrlComponent::Port,
                    UrlComponent::Query,
                    UrlComponent::Fragment,
                ],
            },
        )])
        .unwrap();
        assert_eq!(
            remove
                .apply(&ClipboardItem::from_text(
                    "https://example.com:8443/a/b?id=1#part",
                ))
                .unwrap()
                .after
                .text(),
            Some("https://example.com/")
        );
    }

    #[test]
    fn single_child_all_matching_chain_is_compact_and_preserves_wrappers() {
        let mut nested = RawRule {
            id: "leaf".into(),
            from: Some("cat".into()),
            to: Some("dog".into()),
            ..RawRule::default()
        };
        for depth in (0..10).rev() {
            nested = RawRule {
                kind: Some("ruleset".into()),
                id: format!("wrapper-{depth}"),
                mode: Some(RulesetMode::AllMatching),
                rules: vec![nested],
                ..RawRule::default()
            };
        }
        let mut engine = RuleEngine::compile(vec![nested]).unwrap();

        assert_eq!(engine.rule_count(), 11);
        assert_eq!(engine.compiled_node_count(), 1);
        assert_eq!(engine.compact_chain_depth(), 10);
        let result = engine
            .try_apply(&ClipboardItem::from_text("cat"))
            .unwrap()
            .unwrap();
        assert_eq!(result.after.text(), Some("dog"));
        assert_eq!(
            result.applied_rule_ids().collect::<Vec<_>>(),
            [
                "wrapper-0",
                "wrapper-1",
                "wrapper-2",
                "wrapper-3",
                "wrapper-4",
                "wrapper-5",
                "wrapper-6",
                "wrapper-7",
                "wrapper-8",
                "wrapper-9",
                "leaf",
            ]
        );

        let disabled = BTreeSet::from(["wrapper-5".to_string()]);
        assert!(engine
            .try_apply_excluding(&ClipboardItem::from_text("cat"), &disabled)
            .unwrap()
            .is_none());
    }

    #[test]
    fn regexp_rewrites_text() {
        let mut engine = RuleEngine::compile(vec![RawRule {
            id: "shorts-to-watch".into(),
            name: Some("YouTube Shorts".into()),
            from: Some(r"https://www\.youtube\.com/shorts/([\w-]+)".into()),
            to: Some("https://www.youtube.com/watch?v=$1".into()),
            message: Some("$1 converted".into()),
            ..RawRule::default()
        }])
        .unwrap();

        let result = engine
            .apply(&ClipboardItem::from_text(
                "https://www.youtube.com/shorts/abc-123",
            ))
            .unwrap();

        assert_eq!(
            result.after.text(),
            Some("https://www.youtube.com/watch?v=abc-123")
        );
        assert_eq!(result.message, Some("abc-123 converted".into()));
        assert_eq!(result.applied_rules[0].label(), "YouTube Shorts");
    }

    #[test]
    fn required_formats_are_canonical_and_include_nested_rules() {
        let engine = RuleEngine::compile(vec![
            RawRule {
                id: "html".into(),
                from: Some("cat".into()),
                to: Some("dog".into()),
                formats: vec!["html".into()],
                ..RawRule::default()
            },
            RawRule {
                kind: Some("url-cleanup".into()),
                id: "url".into(),
                formats: vec!["url".into()],
                remove_query_params: vec!["utm_source".into()],
                ..RawRule::default()
            },
            RawRule {
                kind: Some("ruleset".into()),
                id: "nested".into(),
                formats: vec!["file".into()],
                rules: vec![RawRule {
                    id: "rtf".into(),
                    from: Some("cat".into()),
                    to: Some("dog".into()),
                    formats: vec!["rtf".into()],
                    ..RawRule::default()
                }],
                ..RawRule::default()
            },
        ])
        .unwrap();

        assert_eq!(
            engine
                .required_formats()
                .iter()
                .map(ClipboardFormat::as_str)
                .collect::<Vec<_>>(),
            ["file-url", "html", "rtf", "url"]
        );
    }

    #[test]
    fn regexp_message_expands_template_without_leaking_surrounding_text() {
        let mut engine = RuleEngine::compile(vec![RawRule {
            id: "shorts-to-watch".into(),
            from: Some(r"https://www\.youtube\.com/shorts/([\w-]+)".into()),
            to: Some("https://www.youtube.com/watch?v=$1".into()),
            message: Some("$1 converted".into()),
            ..RawRule::default()
        }])
        .unwrap();

        let result = engine
            .apply(&ClipboardItem::from_text(
                "check https://www.youtube.com/shorts/abc-123 now",
            ))
            .unwrap();

        assert_eq!(
            result.after.text(),
            Some("check https://www.youtube.com/watch?v=abc-123 now")
        );
        assert_eq!(result.message, Some("abc-123 converted".into()));
    }

    #[test]
    fn rule_app_blacklist_skips_matching_source_app() {
        let mut engine = RuleEngine::compile(vec![RawRule {
            id: "cat-to-dog".into(),
            from: Some("cat".into()),
            to: Some("dog".into()),
            apps: vec!["com.example.Blocked".into()],
            app_mode: Some(AppMode::Blacklist),
            ..RawRule::default()
        }])
        .unwrap();

        let content = ClipboardItem::from_text("cat").with_source_app(ClipboardSourceApp::new(
            Some("com.example.Blocked".into()),
            Some("Blocked".into()),
        ));

        assert!(engine.apply(&content).is_none());
    }

    #[test]
    fn rule_app_whitelist_allows_matching_source_app() {
        let mut engine = RuleEngine::compile(vec![RawRule {
            id: "cat-to-dog".into(),
            from: Some("cat".into()),
            to: Some("dog".into()),
            apps: vec!["Allowed".into()],
            app_mode: Some(AppMode::Whitelist),
            ..RawRule::default()
        }])
        .unwrap();

        let content = ClipboardItem::from_text("cat").with_source_app(ClipboardSourceApp::new(
            Some("com.example.Allowed".into()),
            Some("Allowed".into()),
        ));

        assert_eq!(engine.apply(&content).unwrap().after.text(), Some("dog"));
    }

    #[test]
    fn rule_app_filter_requires_mode() {
        let err = RuleEngine::compile(vec![RawRule {
            id: "cat-to-dog".into(),
            from: Some("cat".into()),
            to: Some("dog".into()),
            apps: vec!["com.example.App".into()],
            ..RawRule::default()
        }])
        .unwrap_err();

        assert!(err.to_string().contains("app_mode"));
    }

    #[test]
    fn rule_id_is_required() {
        let err = RuleEngine::compile(vec![RawRule {
            id: String::new(),
            from: Some("cat".into()),
            to: Some("dog".into()),
            ..RawRule::default()
        }])
        .unwrap_err();

        assert!(err.to_string().contains("cannot be empty"));
    }

    #[test]
    fn url_cleanup_removes_query_params_by_pattern() {
        let mut engine = RuleEngine::compile(vec![RawRule {
            kind: Some("url-cleanup".into()),
            id: "remove-patterns".into(),
            remove_query_param_patterns: vec!["dbkanal_[0-9]{3}".into(), "at_[a-z_]+".into()],
            ..RawRule::default()
        }])
        .unwrap();

        let result = engine
            .apply(&ClipboardItem::from_text(
                "https://example.com/page?dbkanal_123=x&keep=1&at_source=news",
            ))
            .unwrap();

        assert_eq!(result.after.text(), Some("https://example.com/page?keep=1"));
    }

    #[test]
    fn url_cleanup_preserves_encoding_of_kept_query_params() {
        let mut engine = RuleEngine::compile(vec![RawRule {
            kind: Some("url-cleanup".into()),
            id: "remove-tracking".into(),
            remove_query_params: vec!["utm_source".into()],
            ..RawRule::default()
        }])
        .unwrap();

        let result = engine
            .apply(&ClipboardItem::from_text(
                "https://example.com/search?q=hello%20world&flag&data=a/b:c&utm_source=x",
            ))
            .unwrap();

        assert_eq!(
            result.after.text(),
            Some("https://example.com/search?q=hello%20world&flag&data=a/b:c")
        );
    }

    #[test]
    fn url_cleanup_matches_encoded_keys_and_keeps_fragment() {
        let mut engine = RuleEngine::compile(vec![RawRule {
            kind: Some("url-cleanup".into()),
            id: "remove-tracking".into(),
            remove_query_params: vec!["utm source".into()],
            ..RawRule::default()
        }])
        .unwrap();

        let result = engine
            .apply(&ClipboardItem::from_text(
                "https://example.com/page?utm%20source=x&keep=1#section",
            ))
            .unwrap();

        assert_eq!(
            result.after.text(),
            Some("https://example.com/page?keep=1#section")
        );
    }

    #[test]
    fn all_matching_skips_non_matches() {
        let mut engine = RuleEngine::compile(vec![RawRule {
            kind: Some("ruleset".into()),
            id: "all-matching".into(),
            mode: Some(RulesetMode::AllMatching),
            rules: vec![
                RawRule {
                    id: "nope".into(),
                    from: Some("nope".into()),
                    to: Some("x".into()),
                    ..RawRule::default()
                },
                RawRule {
                    id: "cat-to-dog".into(),
                    from: Some("cat".into()),
                    to: Some("dog".into()),
                    ..RawRule::default()
                },
            ],
            ..RawRule::default()
        }])
        .unwrap();

        assert_eq!(
            engine
                .apply(&ClipboardItem::from_text("cat"))
                .unwrap()
                .after
                .text(),
            Some("dog")
        );
    }

    #[test]
    fn all_stops_on_non_match() {
        let mut engine = RuleEngine::compile(vec![RawRule {
            kind: Some("ruleset".into()),
            id: "all".into(),
            mode: Some(RulesetMode::All),
            rules: vec![
                RawRule {
                    id: "nope".into(),
                    from: Some("nope".into()),
                    to: Some("x".into()),
                    ..RawRule::default()
                },
                RawRule {
                    id: "cat-to-dog".into(),
                    from: Some("cat".into()),
                    to: Some("dog".into()),
                    ..RawRule::default()
                },
            ],
            ..RawRule::default()
        }])
        .unwrap();

        assert!(engine.apply(&ClipboardItem::from_text("cat")).is_none());
    }

    #[test]
    fn while_matching_keeps_the_matching_prefix() {
        let mut engine = RuleEngine::compile(vec![RawRule {
            kind: Some("ruleset".into()),
            id: "while-matching".into(),
            mode: Some(RulesetMode::WhileMatching),
            rules: vec![
                RawRule {
                    id: "cat-to-dog".into(),
                    from: Some("cat".into()),
                    to: Some("dog".into()),
                    ..RawRule::default()
                },
                RawRule {
                    id: "bird-to-fish".into(),
                    from: Some("bird".into()),
                    to: Some("fish".into()),
                    ..RawRule::default()
                },
                RawRule {
                    id: "dog-to-fox".into(),
                    from: Some("dog".into()),
                    to: Some("fox".into()),
                    ..RawRule::default()
                },
            ],
            ..RawRule::default()
        }])
        .unwrap();

        assert_eq!(
            engine
                .apply(&ClipboardItem::from_text("cat"))
                .unwrap()
                .after
                .text(),
            Some("dog")
        );

        let disabled = BTreeSet::from(["bird-to-fish".to_string()]);
        assert_eq!(
            engine
                .apply_excluding(&ClipboardItem::from_text("cat"), &disabled)
                .unwrap()
                .after
                .text(),
            Some("fox")
        );
    }

    #[test]
    fn regexp_flags_control_matching_and_replacement() {
        let mut engine = RuleEngine::compile(vec![RawRule {
            id: "multiline-case-insensitive".into(),
            from: Some("^cat$".into()),
            to: Some("dog".into()),
            flags: Some("im".into()),
            ..RawRule::default()
        }])
        .unwrap();

        assert_eq!(
            engine
                .apply(&ClipboardItem::from_text("CAT\ncat"))
                .unwrap()
                .after
                .text(),
            Some("dog\ndog")
        );
    }

    #[test]
    fn text_transform_writes_only_the_new_text_payload() {
        let mut item = ClipboardItem::from_text("cat");
        item.set_bytes(
            ClipboardFormat::new("public.png"),
            vec![0x89, b'P', b'N', b'G'],
        );

        let mut engine = RuleEngine::compile(vec![RawRule {
            id: "cat-to-dog".into(),
            from: Some("cat".into()),
            to: Some("dog".into()),
            ..RawRule::default()
        }])
        .unwrap();

        let result = engine.apply(&item).unwrap();
        assert_eq!(result.after.text(), Some("dog"));
        assert_eq!(
            result.after.bytes(&ClipboardFormat::new("public.png")),
            None
        );
        assert!(result.after.representations().is_empty());
    }

    #[test]
    fn text_transform_drops_all_stale_semantics_and_native_payloads() {
        let mut item = ClipboardItem::from_text("cat");
        item.set_html("<b>cat</b>");
        item.set_rtf(r"{\rtf1 cat}");
        item.set_bytes(ClipboardFormat::new("public.png"), vec![1, 2, 3]);
        let mut engine = RuleEngine::compile(vec![RawRule {
            id: "cat-to-dog".into(),
            from: Some("cat".into()),
            to: Some("dog".into()),
            ..RawRule::default()
        }])
        .unwrap();

        let result = engine.apply(&item).unwrap();

        assert_eq!(result.after.text(), Some("dog"));
        assert_eq!(result.after.html(), None);
        assert_eq!(result.after.rtf(), None);
        assert_eq!(
            result.after.bytes(&ClipboardFormat::new("public.png")),
            None
        );
        assert!(result.after.representations().is_empty());
    }

    #[test]
    fn text_transform_uses_the_first_available_configured_format() {
        let mut item = ClipboardItem::from_text("cat");
        item.set_html("<b>cat</b>");
        item.set_rtf(r"{\rtf1 cat}");
        let mut engine = RuleEngine::compile(vec![RawRule {
            id: "cat-to-dog".into(),
            from: Some("cat".into()),
            to: Some("dog".into()),
            formats: vec!["html".into(), "text".into(), "rtf".into()],
            ..RawRule::default()
        }])
        .unwrap();

        let result = engine.apply(&item).unwrap();

        assert_eq!(result.after.text(), Some("<b>dog</b>"));
        assert_eq!(result.after.html(), None);
        assert_eq!(result.after.rtf(), None);
    }

    #[test]
    fn text_transform_falls_back_to_the_next_missing_format() {
        let mut engine = RuleEngine::compile(vec![RawRule {
            id: "cat-to-dog".into(),
            from: Some("cat".into()),
            to: Some("dog".into()),
            formats: vec!["html".into(), "text".into()],
            ..RawRule::default()
        }])
        .unwrap();

        let result = engine.apply(&ClipboardItem::from_text("cat")).unwrap();

        assert_eq!(result.after.text(), Some("dog"));
    }

    #[test]
    fn text_transform_does_not_retry_after_an_available_format_does_not_match() {
        let mut item = ClipboardItem::from_text("cat");
        item.set_html("<b>bird</b>");
        let mut engine = RuleEngine::compile(vec![RawRule {
            id: "cat-to-dog".into(),
            from: Some("cat".into()),
            to: Some("dog".into()),
            formats: vec!["html".into(), "text".into()],
            ..RawRule::default()
        }])
        .unwrap();

        assert!(engine.apply(&item).is_none());
    }

    #[test]
    fn disabled_rule_is_skipped_without_cancelling_the_pipeline() {
        let mut engine = RuleEngine::compile(vec![
            RawRule {
                id: "cat-to-dog".into(),
                from: Some("cat".into()),
                to: Some("dog".into()),
                ..RawRule::default()
            },
            RawRule {
                id: "dog-to-bird".into(),
                from: Some("dog".into()),
                to: Some("bird".into()),
                ..RawRule::default()
            },
        ])
        .unwrap();
        let disabled = ["dog-to-bird".to_string()].into();

        let result = engine
            .apply_excluding(&ClipboardItem::from_text("cat"), &disabled)
            .unwrap();

        assert_eq!(result.after.text(), Some("dog"));
        assert_eq!(
            result.applied_rule_ids().collect::<Vec<_>>(),
            ["cat-to-dog"]
        );
    }
}
