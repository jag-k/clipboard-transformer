//! Built-in nested ruleset implementation.

use super::*;

pub(super) struct RulesetProvider;

impl provider::RuleProvider for RulesetProvider {
    fn kind(&self) -> &str {
        "ruleset"
    }

    fn compile(
        &self,
        mut raw: RawRule,
        providers: &provider::RuleProviderRegistry,
    ) -> Result<Box<dyn provider::CompiledRule>> {
        if raw.rules.is_empty() {
            bail!("ruleset requires nested rules");
        }
        if raw.mode.unwrap_or_default() == RulesetMode::AllMatching && raw.rules.len() == 1 {
            let mut wrappers = Vec::new();
            loop {
                wrappers.push(provider::RuleCommon::compile(&raw)?);
                let child = raw.rules.pop().expect("single ruleset child");
                if child.kind.as_deref() == Some("ruleset")
                    && child.mode.unwrap_or_default() == RulesetMode::AllMatching
                    && child.rules.len() == 1
                {
                    raw = child;
                    continue;
                }
                let child = providers.compile_rule(child)?;
                return Ok(Box::new(AllMatchingChain { wrappers, child }));
            }
        }
        let common = provider::RuleCommon::compile(&raw)?;
        let mode = raw.mode.unwrap_or_default();
        let ruleset = providers.compile_rules_for_mode(raw.rules, mode)?;
        Ok(Box::new(provider::ItemRuleAdapter::new(
            common,
            Box::new(RulesetTransform { ruleset }),
        )))
    }
}

struct AllMatchingChain {
    wrappers: Vec<provider::RuleCommon>,
    child: Option<Box<dyn provider::CompiledRule>>,
}

impl provider::CompiledRule for AllMatchingChain {
    fn id(&self) -> &str {
        self.wrappers[0].id()
    }

    fn apply(
        &mut self,
        input: &ClipboardItem,
        disabled_rule_ids: &BTreeSet<String>,
    ) -> Result<Option<RuleOutcome>> {
        for wrapper in &self.wrappers {
            if disabled_rule_ids.contains(wrapper.id()) || !wrapper.accepts_item(input) {
                return Ok(None);
            }
        }
        let Some(child) = &mut self.child else {
            return Ok(None);
        };
        if disabled_rule_ids.contains(child.id()) {
            return Ok(None);
        }
        let Some(mut outcome) = child.apply(input, disabled_rule_ids)? else {
            return Ok(None);
        };
        for wrapper in self.wrappers.iter().rev() {
            outcome.applied_rules.insert(0, wrapper.applied_rule());
        }
        Ok(Some(outcome))
    }

    fn count(&self) -> usize {
        self.wrappers.len() + self.child.as_ref().map_or(0, |child| child.count())
    }

    fn collect_formats(&self, formats: &mut BTreeSet<ClipboardFormat>) {
        for wrapper in &self.wrappers {
            formats.extend(wrapper.formats().iter().cloned());
        }
        if let Some(child) = &self.child {
            child.collect_formats(formats);
        }
    }

    #[cfg(test)]
    fn compact_chain_depth(&self) -> usize {
        self.wrappers.len()
    }
}

struct RulesetTransform {
    ruleset: provider::CompiledRuleset,
}

impl provider::ItemTransform for RulesetTransform {
    fn transform(
        &mut self,
        input: &ClipboardItem,
        disabled_rule_ids: &BTreeSet<String>,
    ) -> Result<Option<provider::ItemTransformOutput>> {
        self.ruleset.apply(input, disabled_rule_ids)
    }

    fn nested_rule_count(&self) -> usize {
        self.ruleset.rule_count()
    }

    fn collect_formats(&self, formats: &mut BTreeSet<ClipboardFormat>) {
        self.ruleset.collect_formats(formats);
    }
}
