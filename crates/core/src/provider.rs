//! Built-in and injected rule provider compilation.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::OnceLock;

use anyhow::{bail, Result};

use super::{
    normalize_formats, AppMatcher, AppliedRule, ClipboardFormat, ClipboardItem, ClipboardSourceApp,
    ExternalRuleProvider, ExternalTransform, RawRule, RuleOutcome, SkippedExternalRule,
};

pub(super) trait RuleProvider: Send + Sync {
    fn kind(&self) -> &str;

    fn compile(
        &self,
        raw: RawRule,
        providers: &RuleProviderRegistry,
    ) -> Result<Box<dyn CompiledRule>>;
}

pub(super) struct RuleProviderRegistry {
    providers: BTreeMap<String, Box<dyn RuleProvider>>,
    external_kinds: BTreeSet<String>,
    skipped_external: RefCell<Vec<SkippedExternalRule>>,
}

impl RuleProviderRegistry {
    pub(super) fn builtins() -> Self {
        let mut registry = Self {
            providers: BTreeMap::new(),
            external_kinds: BTreeSet::new(),
            skipped_external: RefCell::new(Vec::new()),
        };
        registry.register(Box::new(super::regexp::RegexpProvider));
        registry.register(Box::new(super::url::UrlProvider));
        registry.register(Box::new(super::url::UrlCleanupProvider));
        registry.register(Box::new(super::ruleset::RulesetProvider));
        registry
    }

    pub(super) fn with_external(external: &[Arc<dyn ExternalRuleProvider>]) -> Self {
        let mut registry = Self::builtins();
        for provider in external {
            let kind = provider.kind().to_string();
            debug_assert!(
                !super::is_registered_rule_type(&kind),
                "external rule provider must not shadow a built-in type"
            );
            registry.external_kinds.insert(kind);
            registry.register(Box::new(ExternalProviderAdapter(Arc::clone(provider))));
        }
        registry
    }

    fn register(&mut self, provider: Box<dyn RuleProvider>) {
        let previous = self.providers.insert(provider.kind().to_string(), provider);
        debug_assert!(previous.is_none(), "duplicate rule provider");
    }

    /// Compiles one rule. External-provider failures are recorded and
    /// reported as a skipped rule (`Ok(None)`) so one broken plugin rule
    /// cannot take down the rest of the configuration; built-in failures
    /// remain hard errors because config loading already validated them.
    pub(super) fn compile_rule(&self, raw: RawRule) -> Result<Option<Box<dyn CompiledRule>>> {
        let kind = raw.kind.clone().unwrap_or_else(|| "regexp".to_string());
        let is_external = self.external_kinds.contains(&kind);
        let rule_id = raw.id.clone();
        let compiled = self.compile_rule_strict(raw, &kind);
        match compiled {
            Ok(rule) => Ok(Some(rule)),
            Err(error) if is_external => {
                self.skipped_external
                    .borrow_mut()
                    .push(SkippedExternalRule {
                        id: rule_id,
                        kind,
                        reason: format!("{error:#}"),
                    });
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn compile_rule_strict(&self, raw: RawRule, kind: &str) -> Result<Box<dyn CompiledRule>> {
        if let Some(error) = raw.validation_error.as_deref() {
            bail!("invalid rule configuration: {error}");
        }
        let Some(provider) = self.providers.get(kind) else {
            bail!("unsupported rule type {kind:?}");
        };
        provider.compile(raw, self)
    }

    pub(super) fn compile_rules(&self, raw_rules: Vec<RawRule>) -> Result<CompiledRuleset> {
        self.compile_rules_for_mode(raw_rules, super::RulesetMode::AllMatching)
    }

    pub(super) fn compile_rules_for_mode(
        &self,
        raw_rules: Vec<RawRule>,
        mode: super::RulesetMode,
    ) -> Result<CompiledRuleset> {
        let mut compiled = Vec::new();
        let mut pending_url_cleanup = Vec::new();
        let flush =
            |pending: &mut Vec<RawRule>, compiled: &mut Vec<Box<dyn CompiledRule>>| -> Result<()> {
                match pending.len() {
                    0 => {}
                    1 => {
                        let raw = pending.pop().expect("one pending URL cleanup rule");
                        if let Some(rule) = self.compile_rule(raw)? {
                            compiled.push(rule);
                        }
                    }
                    _ => compiled.push(super::url::compile_batch(std::mem::take(pending))?),
                }
                Ok(())
            };

        for raw in raw_rules {
            if matches!(raw.kind.as_deref(), Some("url" | "url-cleanup"))
                && raw.validation_error.is_none()
            {
                pending_url_cleanup.push(raw);
            } else {
                flush(&mut pending_url_cleanup, &mut compiled)?;
                if let Some(rule) = self.compile_rule(raw)? {
                    compiled.push(rule);
                }
            }
        }
        flush(&mut pending_url_cleanup, &mut compiled)?;
        Ok(CompiledRuleset::new(mode, compiled))
    }

    #[cfg(test)]
    pub(super) fn compile_rules_linear(&self, raw_rules: Vec<RawRule>) -> Result<CompiledRuleset> {
        self.compile_rules_linear_for_mode(raw_rules, super::RulesetMode::AllMatching)
    }

    #[cfg(test)]
    pub(super) fn compile_rules_linear_for_mode(
        &self,
        raw_rules: Vec<RawRule>,
        mode: super::RulesetMode,
    ) -> Result<CompiledRuleset> {
        let rules = raw_rules
            .into_iter()
            .map(|raw| self.compile_rule(raw))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        Ok(CompiledRuleset::new(mode, rules))
    }

    pub(super) fn take_skipped_external(&self) -> Vec<SkippedExternalRule> {
        std::mem::take(&mut self.skipped_external.borrow_mut())
    }
}

/// Bridges public [`ExternalRuleProvider`] implementations into the internal
/// text/full-item provider registry.
struct ExternalProviderAdapter(Arc<dyn ExternalRuleProvider>);

impl RuleProvider for ExternalProviderAdapter {
    fn kind(&self) -> &str {
        self.0.kind()
    }

    fn compile(
        &self,
        raw: RawRule,
        _providers: &RuleProviderRegistry,
    ) -> Result<Box<dyn CompiledRule>> {
        let mut raw = raw;
        if raw.formats.is_empty() {
            raw.formats = self.0.default_formats().to_vec();
        }
        let common = RuleCommon::compile(&raw)?;
        let settings = raw
            .plugin_settings
            .map(serde_json::Value::Object)
            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
        match self.0.compile(&raw.id, &settings)? {
            ExternalTransform::Text(transform) => Ok(Box::new(TextRuleAdapter::new(
                common,
                Box::new(ExternalTextTransformAdapter(transform)),
            ))),
            ExternalTransform::Item(transform) => Ok(Box::new(ItemRuleAdapter::new(
                common,
                Box::new(ExternalItemTransformAdapter(transform)),
            ))),
        }
    }
}

struct ExternalTextTransformAdapter(Box<dyn super::ExternalTextTransform>);

impl TextTransform for ExternalTextTransformAdapter {
    fn transform(&mut self, input: TextTransformInput<'_>) -> Result<Option<TextTransformOutput>> {
        Ok(self
            .0
            .transform(input.format.as_str(), input.value, input.source_app)?
            .map(|output| TextTransformOutput {
                text: output.text,
                message: output.message,
            }))
    }
}

struct ExternalItemTransformAdapter(Box<dyn super::ExternalItemTransform>);

impl ItemTransform for ExternalItemTransformAdapter {
    fn transform(
        &mut self,
        input: &ClipboardItem,
        _disabled_rule_ids: &BTreeSet<String>,
    ) -> Result<Option<ItemTransformOutput>> {
        Ok(self.0.transform(input)?.map(|output| ItemTransformOutput {
            content: output.item,
            applied_rules: Vec::new(),
            message: output.message,
        }))
    }
}

pub(super) trait CompiledRule: Send {
    fn id(&self) -> &str;

    fn apply(
        &mut self,
        input: &ClipboardItem,
        disabled_rule_ids: &BTreeSet<String>,
    ) -> Result<Option<RuleOutcome>>;

    fn apply_in_sequence(
        &mut self,
        _mode: super::RulesetMode,
        input: &ClipboardItem,
        disabled_rule_ids: &BTreeSet<String>,
    ) -> Result<SequenceRuleOutcome> {
        Ok(SequenceRuleOutcome {
            outcome: self.apply(input, disabled_rule_ids)?,
            stop: false,
        })
    }

    fn count(&self) -> usize {
        1
    }

    fn collect_formats(&self, formats: &mut BTreeSet<ClipboardFormat>);

    #[cfg(test)]
    fn compact_chain_depth(&self) -> usize {
        0
    }
}

pub(super) struct SequenceRuleOutcome {
    pub(super) outcome: Option<RuleOutcome>,
    pub(super) stop: bool,
}

pub(super) struct CompiledRuleset {
    mode: super::RulesetMode,
    rules: Vec<Box<dyn CompiledRule>>,
}

impl CompiledRuleset {
    pub(super) fn new(mode: super::RulesetMode, rules: Vec<Box<dyn CompiledRule>>) -> Self {
        Self { mode, rules }
    }

    pub(super) fn apply(
        &mut self,
        input: &ClipboardItem,
        disabled_rule_ids: &BTreeSet<String>,
    ) -> Result<Option<ItemTransformOutput>> {
        apply_ruleset(&mut self.rules, self.mode, input, disabled_rule_ids)
    }

    pub(super) fn rule_count(&self) -> usize {
        self.rules.iter().map(|rule| rule.count()).sum()
    }

    pub(super) fn collect_formats(&self, formats: &mut BTreeSet<ClipboardFormat>) {
        for rule in &self.rules {
            rule.collect_formats(formats);
        }
    }

    #[cfg(test)]
    pub(super) fn compiled_node_count(&self) -> usize {
        self.rules.len()
    }

    #[cfg(test)]
    pub(super) fn compact_chain_depth(&self) -> usize {
        self.rules
            .iter()
            .map(|rule| rule.compact_chain_depth())
            .sum()
    }
}

pub(super) struct RuleCommon {
    id: String,
    name: Option<String>,
    app_matcher: AppMatcher,
    formats: RuleFormats,
}

enum RuleFormats {
    Text,
    Explicit(Vec<ClipboardFormat>),
}

impl RuleFormats {
    fn compile(formats: &[String]) -> Result<Self> {
        if formats.is_empty() {
            Ok(Self::Text)
        } else {
            Ok(Self::Explicit(normalize_formats(formats.to_vec())?))
        }
    }

    fn as_slice(&self) -> &[ClipboardFormat] {
        match self {
            Self::Text => {
                std::slice::from_ref(DEFAULT_TEXT_FORMAT.get_or_init(ClipboardFormat::text))
            }
            Self::Explicit(formats) => formats,
        }
    }
}

static DEFAULT_TEXT_FORMAT: OnceLock<ClipboardFormat> = OnceLock::new();

impl RuleCommon {
    pub(super) fn compile(raw: &RawRule) -> Result<Self> {
        Ok(Self {
            id: super::required_rule_id(raw.id.clone())?,
            name: raw.name.clone(),
            app_matcher: AppMatcher::compile(raw.apps.clone(), raw.app_mode)?,
            formats: RuleFormats::compile(&raw.formats)?,
        })
    }

    pub(super) fn id(&self) -> &str {
        &self.id
    }

    pub(super) fn allows(&self, input: &ClipboardItem) -> bool {
        self.app_matcher.allows(input)
    }

    pub(super) fn formats(&self) -> &[ClipboardFormat] {
        self.formats.as_slice()
    }

    pub(super) fn accepts_item(&self, input: &ClipboardItem) -> bool {
        self.allows(input)
            && self
                .formats()
                .iter()
                .any(|format| format.as_str() == "*" || input.bytes(format).is_some())
    }

    pub(super) fn applied_rule(&self) -> AppliedRule {
        AppliedRule {
            id: self.id.clone(),
            name: self.name.clone(),
        }
    }
}

pub(super) struct TextTransformInput<'a> {
    pub(super) format: &'a ClipboardFormat,
    pub(super) value: &'a str,
    pub(super) source_app: Option<&'a ClipboardSourceApp>,
}

pub(super) struct TextTransformOutput {
    pub(super) text: String,
    pub(super) message: Option<String>,
}

pub(super) trait TextTransform: Send {
    fn transform(&mut self, input: TextTransformInput<'_>) -> Result<Option<TextTransformOutput>>;
}

pub(super) struct TextRuleAdapter {
    common: RuleCommon,
    transform: Box<dyn TextTransform>,
}

impl TextRuleAdapter {
    pub(super) fn new(common: RuleCommon, transform: Box<dyn TextTransform>) -> Self {
        Self { common, transform }
    }
}

impl CompiledRule for TextRuleAdapter {
    fn id(&self) -> &str {
        &self.common.id
    }

    fn apply(
        &mut self,
        input: &ClipboardItem,
        _disabled_rule_ids: &BTreeSet<String>,
    ) -> Result<Option<RuleOutcome>> {
        if !self.common.app_matcher.allows(input) {
            return Ok(None);
        }
        let Some((format, value)) = self
            .common
            .formats
            .as_slice()
            .iter()
            .find_map(|format| input.get(format).map(|value| (format, value)))
        else {
            return Ok(None);
        };
        let Some(output) = self.transform.transform(TextTransformInput {
            format,
            value,
            source_app: input.source_app(),
        })?
        else {
            return Ok(None);
        };
        if output.text == value {
            return Ok(None);
        }

        let mut content = input.clone();
        content.replace_with_text(output.text);
        Ok(Some(RuleOutcome {
            content,
            applied_rules: vec![self.common.applied_rule()],
            message: output.message,
        }))
    }

    fn collect_formats(&self, formats: &mut BTreeSet<ClipboardFormat>) {
        formats.extend(self.common.formats.as_slice().iter().cloned());
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ItemTransformOutput {
    pub(super) content: ClipboardItem,
    pub(super) applied_rules: Vec<AppliedRule>,
    pub(super) message: Option<String>,
}

pub(super) trait ItemTransform: Send {
    fn transform(
        &mut self,
        input: &ClipboardItem,
        disabled_rule_ids: &BTreeSet<String>,
    ) -> Result<Option<ItemTransformOutput>>;

    fn nested_rule_count(&self) -> usize {
        0
    }

    fn collect_formats(&self, _formats: &mut BTreeSet<ClipboardFormat>) {}
}

pub(super) struct ItemRuleAdapter {
    common: RuleCommon,
    transform: Box<dyn ItemTransform>,
}

impl ItemRuleAdapter {
    pub(super) fn new(common: RuleCommon, transform: Box<dyn ItemTransform>) -> Self {
        Self { common, transform }
    }
}

impl CompiledRule for ItemRuleAdapter {
    fn id(&self) -> &str {
        &self.common.id
    }

    fn apply(
        &mut self,
        input: &ClipboardItem,
        disabled_rule_ids: &BTreeSet<String>,
    ) -> Result<Option<RuleOutcome>> {
        if !self.common.app_matcher.allows(input) {
            return Ok(None);
        }
        if !self
            .common
            .formats
            .as_slice()
            .iter()
            .any(|format| format.as_str() == "*" || input.bytes(format).is_some())
        {
            return Ok(None);
        }
        let Some(mut output) = self.transform.transform(input, disabled_rule_ids)? else {
            return Ok(None);
        };
        if output.content == *input {
            return Ok(None);
        }
        output.applied_rules.insert(0, self.common.applied_rule());
        Ok(Some(RuleOutcome {
            content: output.content,
            applied_rules: output.applied_rules,
            message: output.message,
        }))
    }

    fn count(&self) -> usize {
        1 + self.transform.nested_rule_count()
    }

    fn collect_formats(&self, formats: &mut BTreeSet<ClipboardFormat>) {
        formats.extend(self.common.formats.as_slice().iter().cloned());
        self.transform.collect_formats(formats);
    }
}

pub(super) fn apply_ruleset(
    rules: &mut [Box<dyn CompiledRule>],
    mode: super::RulesetMode,
    input: &ClipboardItem,
    disabled_rule_ids: &BTreeSet<String>,
) -> Result<Option<ItemTransformOutput>> {
    match mode {
        super::RulesetMode::AllMatching => {
            let mut current = input.clone();
            let mut applied = Vec::new();
            let mut message = None;
            for rule in rules {
                if disabled_rule_ids.contains(rule.id()) {
                    continue;
                }
                let result = rule.apply_in_sequence(mode, &current, disabled_rule_ids)?;
                if let Some(outcome) = result.outcome {
                    current = outcome.content;
                    applied.extend(outcome.applied_rules);
                    message = outcome.message.or(message);
                }
                if result.stop {
                    break;
                }
            }
            Ok((current != *input).then_some(ItemTransformOutput {
                content: current,
                applied_rules: applied,
                message,
            }))
        }
        super::RulesetMode::WhileMatching => {
            let mut current = input.clone();
            let mut applied = Vec::new();
            let mut message = None;
            for rule in rules {
                if disabled_rule_ids.contains(rule.id()) {
                    continue;
                }
                let result = rule.apply_in_sequence(mode, &current, disabled_rule_ids)?;
                let Some(outcome) = result.outcome else {
                    break;
                };
                current = outcome.content;
                applied.extend(outcome.applied_rules);
                message = outcome.message.or(message);
                if result.stop {
                    break;
                }
            }
            Ok((current != *input).then_some(ItemTransformOutput {
                content: current,
                applied_rules: applied,
                message,
            }))
        }
        super::RulesetMode::All => {
            let mut current = input.clone();
            let mut applied = Vec::new();
            let mut message = None;
            for rule in rules {
                if disabled_rule_ids.contains(rule.id()) {
                    continue;
                }
                let result = rule.apply_in_sequence(mode, &current, disabled_rule_ids)?;
                let Some(outcome) = result.outcome else {
                    return Ok(None);
                };
                current = outcome.content;
                applied.extend(outcome.applied_rules);
                message = outcome.message.or(message);
                if result.stop {
                    break;
                }
            }
            Ok((current != *input).then_some(ItemTransformOutput {
                content: current,
                applied_rules: applied,
                message,
            }))
        }
        super::RulesetMode::First => {
            for rule in rules {
                if disabled_rule_ids.contains(rule.id()) {
                    continue;
                }
                let result = rule.apply_in_sequence(mode, input, disabled_rule_ids)?;
                if let Some(outcome) = result.outcome {
                    return Ok(Some(ItemTransformOutput {
                        content: outcome.content,
                        applied_rules: outcome.applied_rules,
                        message: outcome.message,
                    }));
                }
            }
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ReplaceWithText;

    impl ItemTransform for ReplaceWithText {
        fn transform(
            &mut self,
            _input: &ClipboardItem,
            _disabled_rule_ids: &BTreeSet<String>,
        ) -> Result<Option<ItemTransformOutput>> {
            Ok(Some(ItemTransformOutput {
                content: ClipboardItem::from_text("changed"),
                applied_rules: Vec::new(),
                message: None,
            }))
        }
    }

    #[test]
    fn item_transform_format_filter_accepts_binary_representations() {
        let raw = RawRule {
            id: "binary-item".into(),
            formats: vec!["public.png".into()],
            ..RawRule::default()
        };
        let mut rule = ItemRuleAdapter::new(
            RuleCommon::compile(&raw).unwrap(),
            Box::new(ReplaceWithText),
        );
        let mut input = ClipboardItem::new();
        input.set_bytes(
            ClipboardFormat::new("public.png"),
            vec![0x89, b'P', b'N', b'G'],
        );

        let output = rule.apply(&input, &BTreeSet::new()).unwrap().unwrap();

        assert_eq!(output.content.text(), Some("changed"));
    }
}
