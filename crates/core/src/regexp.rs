//! Built-in regular-expression rule implementation.

use anyhow::{bail, Context, Result};
use regex::{Regex, RegexBuilder};

use super::{
    provider::{
        RuleCommon, RuleProvider, RuleProviderRegistry, TextRuleAdapter, TextTransform,
        TextTransformInput, TextTransformOutput,
    },
    RawRule,
};

pub(super) struct RegexpProvider;

impl RuleProvider for RegexpProvider {
    fn kind(&self) -> &str {
        "regexp"
    }

    fn compile(
        &self,
        raw: RawRule,
        _providers: &RuleProviderRegistry,
    ) -> Result<Box<dyn super::provider::CompiledRule>> {
        let common = RuleCommon::compile(&raw)?;
        Ok(Box::new(TextRuleAdapter::new(
            common,
            Box::new(RegexpTransform::compile(raw)?),
        )))
    }
}

struct RegexpTransform {
    from: Regex,
    to: String,
    message: Option<String>,
}

impl RegexpTransform {
    fn compile(raw: RawRule) -> Result<Self> {
        let from = raw.from.as_deref().context("regexp rule requires from")?;
        let to = raw.to.context("regexp rule requires to")?;
        Ok(Self {
            from: compile_regexp(from, raw.flags.as_deref())?,
            to,
            message: raw.message,
        })
    }
}

impl TextTransform for RegexpTransform {
    fn transform(&mut self, input: TextTransformInput<'_>) -> Result<Option<TextTransformOutput>> {
        let _ = (input.format, input.source_app);
        if !self.from.is_match(input.value) {
            return Ok(None);
        }
        let replacement = self
            .from
            .replace_all(input.value, self.to.as_str())
            .to_string();
        if replacement == input.value {
            return Ok(None);
        }
        let mut message = None;
        if let Some(template) = &self.message {
            if let Some(captures) = self.from.captures(input.value) {
                let mut expanded = String::new();
                captures.expand(template, &mut expanded);
                message = Some(expanded);
            }
        }
        Ok(Some(TextTransformOutput {
            text: replacement,
            message,
        }))
    }
}

fn compile_regexp(pattern: &str, flags: Option<&str>) -> Result<Regex> {
    let mut builder = RegexBuilder::new(pattern);
    let mut seen = std::collections::BTreeSet::new();
    for flag in flags.unwrap_or_default().chars() {
        if !seen.insert(flag) {
            bail!("duplicate regexp flag {flag:?} in {flags:?}");
        }
        match flag {
            'i' => builder.case_insensitive(true),
            'm' => builder.multi_line(true),
            's' => builder.dot_matches_new_line(true),
            'U' => builder.swap_greed(true),
            'x' => builder.ignore_whitespace(true),
            'u' => builder.unicode(true),
            unsupported => bail!(
                "unsupported regexp flag {unsupported:?}; supported flags are i, m, s, U, x, and u"
            ),
        };
    }
    builder
        .build()
        .with_context(|| format!("invalid regexp {pattern:?}"))
}
