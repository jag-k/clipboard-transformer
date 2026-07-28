//! Built-in structural URL rule implementations.

use super::*;
use ::url::{form_urlencoded, Url};
use regex::{Regex, RegexBuilder};

pub(super) struct UrlProvider;
pub(super) struct UrlCleanupProvider;

const BATCH_ID: &str = "<url-batch>";

impl provider::RuleProvider for UrlProvider {
    fn kind(&self) -> &str {
        "url"
    }

    fn compile(
        &self,
        raw: RawRule,
        _providers: &provider::RuleProviderRegistry,
    ) -> Result<Box<dyn provider::CompiledRule>> {
        compile_single(raw)
    }
}

impl provider::RuleProvider for UrlCleanupProvider {
    fn kind(&self) -> &str {
        "url-cleanup"
    }

    fn compile(
        &self,
        raw: RawRule,
        _providers: &provider::RuleProviderRegistry,
    ) -> Result<Box<dyn provider::CompiledRule>> {
        compile_single(raw)
    }
}

fn compile_single(raw: RawRule) -> Result<Box<dyn provider::CompiledRule>> {
    let common = provider::RuleCommon::compile(&raw)?;
    Ok(Box::new(provider::TextRuleAdapter::new(
        common,
        Box::new(CompiledUrlRuleTransform::compile(raw)?),
    )))
}

pub(super) fn compile_batch(raw_rules: Vec<RawRule>) -> Result<Box<dyn provider::CompiledRule>> {
    let rules = raw_rules
        .into_iter()
        .map(|raw| {
            let common = provider::RuleCommon::compile(&raw)?;
            let transform = CompiledUrlRuleTransform::compile(raw)?;
            Ok(CompiledUrlRule { common, transform })
        })
        .collect::<Result<Vec<_>>>()?;
    let shares_text_url = rules
        .iter()
        .all(|rule| rule.common.formats() == std::slice::from_ref(&ClipboardFormat::text()));
    Ok(Box::new(UrlBatch {
        rules,
        shares_text_url,
    }))
}

struct CompiledUrlRule {
    common: provider::RuleCommon,
    transform: CompiledUrlRuleTransform,
}

impl CompiledUrlRule {
    fn apply(
        &mut self,
        input: &ClipboardItem,
        disabled_rule_ids: &BTreeSet<String>,
    ) -> Result<Option<RuleOutcome>> {
        if disabled_rule_ids.contains(self.common.id()) || !self.common.allows(input) {
            return Ok(None);
        }
        let Some(value) = self
            .common
            .formats()
            .iter()
            .find_map(|format| input.get(format))
        else {
            return Ok(None);
        };
        let Some(text) = self.transform.transform_url(value) else {
            return Ok(None);
        };
        if text == value {
            return Ok(None);
        }
        let mut content = input.clone();
        content.replace_with_text(text);
        Ok(Some(RuleOutcome {
            content,
            applied_rules: vec![self.common.applied_rule()],
            message: self.transform.message.clone(),
        }))
    }
}

struct UrlBatch {
    rules: Vec<CompiledUrlRule>,
    shares_text_url: bool,
}

impl provider::CompiledRule for UrlBatch {
    fn id(&self) -> &str {
        BATCH_ID
    }

    fn apply(
        &mut self,
        input: &ClipboardItem,
        disabled_rule_ids: &BTreeSet<String>,
    ) -> Result<Option<RuleOutcome>> {
        Ok(self
            .apply_sequence(RulesetMode::AllMatching, input, disabled_rule_ids)?
            .outcome)
    }

    fn apply_in_sequence(
        &mut self,
        mode: RulesetMode,
        input: &ClipboardItem,
        disabled_rule_ids: &BTreeSet<String>,
    ) -> Result<provider::SequenceRuleOutcome> {
        self.apply_sequence(mode, input, disabled_rule_ids)
    }

    fn count(&self) -> usize {
        self.rules.len()
    }

    fn collect_formats(&self, formats: &mut BTreeSet<ClipboardFormat>) {
        for rule in &self.rules {
            formats.extend(rule.common.formats().iter().cloned());
        }
    }
}

impl UrlBatch {
    fn apply_sequence(
        &mut self,
        mode: RulesetMode,
        input: &ClipboardItem,
        disabled_rule_ids: &BTreeSet<String>,
    ) -> Result<provider::SequenceRuleOutcome> {
        if self.shares_text_url {
            return Ok(self.apply_shared_url(mode, input, disabled_rule_ids));
        }
        self.apply_linear(mode, input, disabled_rule_ids)
    }

    fn apply_linear(
        &mut self,
        mode: RulesetMode,
        input: &ClipboardItem,
        disabled_rule_ids: &BTreeSet<String>,
    ) -> Result<provider::SequenceRuleOutcome> {
        let mut current = input.clone();
        let mut applied_rules = Vec::new();
        let mut message = None;
        let mut stop = false;
        for rule in &mut self.rules {
            if disabled_rule_ids.contains(rule.common.id()) {
                continue;
            }
            if let Some(outcome) = rule.apply(&current, disabled_rule_ids)? {
                current = outcome.content;
                applied_rules.extend(outcome.applied_rules);
                message = outcome.message.or(message);
                if mode == RulesetMode::First {
                    stop = true;
                    break;
                }
            } else {
                match mode {
                    RulesetMode::AllMatching | RulesetMode::First => {}
                    RulesetMode::WhileMatching => {
                        stop = true;
                        break;
                    }
                    RulesetMode::All => {
                        return Ok(provider::SequenceRuleOutcome {
                            outcome: None,
                            stop: false,
                        });
                    }
                }
            }
        }
        Ok(provider::SequenceRuleOutcome {
            outcome: (current != *input).then_some(RuleOutcome {
                content: current,
                applied_rules,
                message,
            }),
            stop,
        })
    }

    fn apply_shared_url(
        &mut self,
        mode: RulesetMode,
        input: &ClipboardItem,
        disabled_rule_ids: &BTreeSet<String>,
    ) -> provider::SequenceRuleOutcome {
        let Some(value) = input.text() else {
            return non_match_for_mode(mode);
        };
        if value.trim() != value {
            return non_match_for_mode(mode);
        }
        let Ok(mut url) = Url::parse(value) else {
            return non_match_for_mode(mode);
        };

        let mut applied_rules = Vec::new();
        let mut message = None;
        let mut stop = false;
        for rule in &mut self.rules {
            if disabled_rule_ids.contains(rule.common.id()) {
                continue;
            }
            let matched =
                rule.common.allows(input) && rule.transform.transform_url_in_place(&mut url);
            if matched {
                applied_rules.push(rule.common.applied_rule());
                message = rule.transform.message.clone().or(message);
                if mode == RulesetMode::First {
                    stop = true;
                    break;
                }
                continue;
            }
            match mode {
                RulesetMode::AllMatching | RulesetMode::First => {}
                RulesetMode::WhileMatching => {
                    stop = true;
                    break;
                }
                RulesetMode::All => {
                    return provider::SequenceRuleOutcome {
                        outcome: None,
                        stop: false,
                    };
                }
            }
        }

        let text = url.to_string();
        let outcome = (text != value).then(|| {
            let mut content = input.clone();
            content.replace_with_text(text);
            RuleOutcome {
                content,
                applied_rules,
                message,
            }
        });
        provider::SequenceRuleOutcome { outcome, stop }
    }
}

fn non_match_for_mode(mode: RulesetMode) -> provider::SequenceRuleOutcome {
    provider::SequenceRuleOutcome {
        outcome: None,
        stop: mode == RulesetMode::WhileMatching,
    }
}

struct CompiledUrlRuleTransform {
    message: Option<String>,
    hosts: Vec<String>,
    transform: CompiledUrlTransform,
}

enum CompiledUrlTransform {
    RemoveQueryParams(RemoveQueryParams),
    RemoveComponents(Vec<UrlComponent>),
    RewriteHost(RewriteValue),
    RewriteScheme(RewriteValue),
    SetQueryParam { name: String, encoded_pair: String },
}

struct RewriteValue {
    from: Option<String>,
    to: String,
}

struct RemoveQueryParams {
    names: Vec<String>,
    prefixes: Vec<String>,
    patterns: Vec<Regex>,
}

impl CompiledUrlRuleTransform {
    fn compile(raw: RawRule) -> Result<Self> {
        let kind = raw.kind.as_deref().unwrap_or("regexp");
        let transform = match kind {
            "url-cleanup" => CompiledUrlTransform::RemoveQueryParams(
                RemoveQueryParams::compile(
                    raw.remove_query_params,
                    raw.remove_query_prefixes,
                    raw.remove_query_param_patterns,
                )
                .context("url-cleanup rule")?,
            ),
            "url" => Self::compile_url_transform(
                raw.url_transform.context("url rule requires transform")?,
            )?,
            _ => bail!("unsupported URL rule type {kind:?}"),
        };
        Ok(Self {
            message: raw.message,
            hosts: raw
                .hosts
                .into_iter()
                .map(|host| host.to_lowercase())
                .collect(),
            transform,
        })
    }

    fn compile_url_transform(transform: UrlTransform) -> Result<CompiledUrlTransform> {
        Ok(match transform {
            UrlTransform::RemoveQueryParams {
                names,
                prefixes,
                patterns,
            } => CompiledUrlTransform::RemoveQueryParams(
                RemoveQueryParams::compile(names, prefixes, patterns)
                    .context("url remove-query-params transform")?,
            ),
            UrlTransform::RemoveComponents { components } => {
                if components.is_empty() {
                    bail!("url remove-components transform requires components");
                }
                CompiledUrlTransform::RemoveComponents(components)
            }
            UrlTransform::RewriteHost { from, to } => {
                if to.trim().is_empty() {
                    bail!("url rewrite-host transform requires to");
                }
                // `Url::set_host` accepts "host:port" and then silently keeps
                // only the host, so an authority here would drop the port
                // without telling anyone. Reject it while the rule compiles.
                if to.contains(':') {
                    bail!(
                        "url rewrite-host transform to must be a bare host without a port, got {to:?}"
                    );
                }
                CompiledUrlTransform::RewriteHost(RewriteValue {
                    from: from.map(|host| host.to_lowercase()),
                    to,
                })
            }
            UrlTransform::RewriteScheme { from, to } => {
                if !matches!(to.as_str(), "http" | "https") {
                    bail!("url rewrite-scheme transform supports only http or https");
                }
                if from
                    .as_deref()
                    .is_some_and(|from| !matches!(from, "http" | "https"))
                {
                    bail!("url rewrite-scheme transform from supports only http or https");
                }
                CompiledUrlTransform::RewriteScheme(RewriteValue { from, to })
            }
            UrlTransform::SetQueryParam { name, value } => {
                if name.is_empty() {
                    bail!("url set-query-param transform requires a non-empty name");
                }
                let encoded_pair = form_urlencoded::Serializer::new(String::new())
                    .append_pair(&name, &value)
                    .finish();
                CompiledUrlTransform::SetQueryParam { name, encoded_pair }
            }
        })
    }
}

impl provider::TextTransform for CompiledUrlRuleTransform {
    fn transform(
        &mut self,
        input: provider::TextTransformInput<'_>,
    ) -> Result<Option<provider::TextTransformOutput>> {
        let _ = (input.format, input.source_app);
        Ok(self
            .transform_url(input.value)
            .map(|text| provider::TextTransformOutput {
                text,
                message: self.message.clone(),
            }))
    }
}

impl CompiledUrlRuleTransform {
    fn transform_url(&self, value: &str) -> Option<String> {
        if value.trim() != value {
            return None;
        }
        let mut url = Url::parse(value).ok()?;
        self.transform_url_in_place(&mut url)
            .then(|| url.to_string())
    }

    fn transform_url_in_place(&self, url: &mut Url) -> bool {
        if !matches!(url.scheme(), "http" | "https") {
            return false;
        }
        if !self.hosts.is_empty() {
            let Some(host) = url.host_str().map(str::to_lowercase) else {
                return false;
            };
            if !self.hosts.iter().any(|allowed| allowed == &host) {
                return false;
            }
        }

        match &self.transform {
            CompiledUrlTransform::RemoveQueryParams(filter) => filter.apply(url),
            CompiledUrlTransform::RemoveComponents(components) => {
                remove_components(url, components)
            }
            CompiledUrlTransform::RewriteHost(rewrite) => {
                let current = url.host_str().map(str::to_lowercase);
                if rewrite
                    .from
                    .as_ref()
                    .is_some_and(|from| current.as_ref() != Some(from))
                    || current.as_deref() == Some(rewrite.to.as_str())
                {
                    return false;
                }
                let before = url.host_str().map(str::to_string);
                url.set_host(Some(&rewrite.to)).is_ok()
                    && url.host_str().map(str::to_string) != before
            }
            CompiledUrlTransform::RewriteScheme(rewrite) => {
                if rewrite
                    .from
                    .as_deref()
                    .is_some_and(|from| url.scheme() != from)
                    || url.scheme() == rewrite.to
                {
                    return false;
                }
                url.set_scheme(&rewrite.to).is_ok()
            }
            CompiledUrlTransform::SetQueryParam { name, encoded_pair } => {
                set_query_param(url, name, encoded_pair)
            }
        }
    }
}

impl RemoveQueryParams {
    fn compile(names: Vec<String>, prefixes: Vec<String>, patterns: Vec<String>) -> Result<Self> {
        if names.is_empty() && prefixes.is_empty() && patterns.is_empty() {
            bail!("requires names, prefixes, or patterns");
        }
        Ok(Self {
            names: names.into_iter().map(|name| name.to_lowercase()).collect(),
            prefixes: prefixes
                .into_iter()
                .map(|prefix| prefix.to_lowercase())
                .collect(),
            patterns: patterns
                .iter()
                .map(|pattern| compile_query_param_pattern(pattern))
                .collect::<Result<Vec<_>>>()?,
        })
    }

    fn apply(&self, url: &mut Url) -> bool {
        // Filter raw query segments instead of round-tripping through
        // query_pairs, which would re-encode kept parameters (`%20` -> `+`,
        // lossy UTF-8 replacement, `?flag` -> `?flag=`).
        let Some(query) = url.query().map(str::to_string) else {
            return false;
        };
        if query.is_empty() {
            return false;
        }
        let segments = query.split('&').collect::<Vec<_>>();
        let kept = segments
            .iter()
            .filter(|segment| !self.matches(raw_query_key(segment)))
            .copied()
            .collect::<Vec<_>>();
        if kept.len() == segments.len() {
            return false;
        }
        set_raw_query_segments(url, &kept);
        true
    }

    fn matches(&self, key: String) -> bool {
        let key = key.to_lowercase();
        self.names.iter().any(|name| name == &key)
            || self.prefixes.iter().any(|prefix| key.starts_with(prefix))
            || self.patterns.iter().any(|pattern| pattern.is_match(&key))
    }
}

fn remove_components(url: &mut Url, components: &[UrlComponent]) -> bool {
    let mut changed = false;
    for component in components {
        match component {
            UrlComponent::Fragment if url.fragment().is_some() => {
                url.set_fragment(None);
                changed = true;
            }
            UrlComponent::Query if url.query().is_some() => {
                url.set_query(None);
                changed = true;
            }
            UrlComponent::Credentials => changed |= remove_credentials(url),
            UrlComponent::Port if url.port().is_some() => {
                if url.set_port(None).is_ok() {
                    changed = true;
                }
            }
            UrlComponent::Path if url.path() != "/" => {
                url.set_path("/");
                changed = true;
            }
            _ => {}
        }
    }
    changed
}

fn remove_credentials(url: &mut Url) -> bool {
    if url.username().is_empty() && url.password().is_none() {
        return false;
    }
    url.set_username("").is_ok() && url.set_password(None).is_ok()
}

fn set_query_param(url: &mut Url, name: &str, encoded_pair: &str) -> bool {
    let original = url.query().unwrap_or_default();
    let mut segments = original
        .split('&')
        .filter(|segment| !segment.is_empty() && raw_query_key(segment) != name)
        .collect::<Vec<_>>();
    segments.push(encoded_pair);
    let query = segments.join("&");
    if original == query {
        return false;
    }
    url.set_query(Some(&query));
    true
}

fn set_raw_query_segments(url: &mut Url, segments: &[&str]) {
    if segments.is_empty() {
        url.set_query(None);
    } else {
        url.set_query(Some(&segments.join("&")));
    }
}

fn raw_query_key(segment: &str) -> String {
    let raw_key = segment.split('=').next().unwrap_or(segment);
    form_urlencoded::parse(raw_key.as_bytes())
        .next()
        .map(|(key, _)| key.into_owned())
        .unwrap_or_default()
}

fn compile_query_param_pattern(pattern: &str) -> Result<Regex> {
    let anchored = format!("^(?:{pattern})$");
    RegexBuilder::new(&anchored)
        .case_insensitive(true)
        .build()
        .with_context(|| format!("invalid query parameter pattern {pattern:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transform(transform: UrlTransform) -> CompiledUrlRuleTransform {
        CompiledUrlRuleTransform::compile(RawRule {
            id: "t".into(),
            kind: Some("url".into()),
            url_transform: Some(transform),
            ..Default::default()
        })
        .expect("compile url transform")
    }

    fn apply(rule: &CompiledUrlRuleTransform, value: &str) -> Option<String> {
        rule.transform_url(value)
    }

    #[test]
    fn rewrite_host_from_guard_matches_case_insensitively_on_both_sides() {
        let rule = transform(UrlTransform::RewriteHost {
            from: Some("OLD.Example.COM".into()),
            to: "new.example.com".into(),
        });

        assert_eq!(
            apply(&rule, "https://Old.EXAMPLE.com/a?b=1"),
            Some("https://new.example.com/a?b=1".into())
        );
    }

    #[test]
    fn rewrite_host_from_guard_leaves_other_hosts_alone() {
        let rule = transform(UrlTransform::RewriteHost {
            from: Some("old.example.com".into()),
            to: "new.example.com".into(),
        });

        assert_eq!(apply(&rule, "https://other.example.com/a"), None);
        // A subdomain is not the guarded host: the guard is exact, not suffix.
        assert_eq!(apply(&rule, "https://sub.old.example.com/a"), None);
    }

    #[test]
    fn rewrite_host_without_guard_still_skips_urls_already_at_the_target() {
        let rule = transform(UrlTransform::RewriteHost {
            from: None,
            to: "example.com".into(),
        });

        assert_eq!(apply(&rule, "https://example.com/a"), None);
        assert_eq!(
            apply(&rule, "https://other.test/a"),
            Some("https://example.com/a".into())
        );
    }

    #[test]
    fn rewrite_host_rejects_a_target_that_is_not_a_bare_host() {
        // `Url::set_host` would accept "example.com:8443" and keep only the
        // host, silently discarding the port the author asked for, so this is
        // rejected while the rule compiles instead.
        let error = CompiledUrlRuleTransform::compile(RawRule {
            id: "t".into(),
            kind: Some("url".into()),
            url_transform: Some(UrlTransform::RewriteHost {
                from: None,
                to: "example.com:8443".into(),
            }),
            ..Default::default()
        })
        .err()
        .expect("a host with a port must not compile");
        assert!(
            error.to_string().contains("bare host"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn rewrite_scheme_from_guard_is_respected_and_validated() {
        let rule = transform(UrlTransform::RewriteScheme {
            from: Some("http".into()),
            to: "https".into(),
        });
        assert_eq!(
            apply(&rule, "http://example.com/a"),
            Some("https://example.com/a".into())
        );
        assert_eq!(apply(&rule, "https://example.com/a"), None);

        let rejected = CompiledUrlRuleTransform::compile(RawRule {
            id: "t".into(),
            kind: Some("url".into()),
            url_transform: Some(UrlTransform::RewriteScheme {
                from: Some("ftp".into()),
                to: "https".into(),
            }),
            ..Default::default()
        });
        assert!(rejected.is_err(), "only http/https are valid `from` values");
    }

    #[test]
    fn remove_components_strips_an_explicit_port_but_not_a_default_one() {
        let rule = transform(UrlTransform::RemoveComponents {
            components: vec![UrlComponent::Port],
        });

        assert_eq!(
            apply(&rule, "https://example.com:8443/a"),
            Some("https://example.com/a".into())
        );
        // The URL parser already drops a scheme's default port, so there is
        // nothing left to remove and the rule reports no match.
        assert_eq!(apply(&rule, "https://example.com:443/a"), None);
        assert_eq!(apply(&rule, "https://example.com/a"), None);
    }

    #[test]
    fn remove_components_path_resets_to_root_and_keeps_other_components() {
        let rule = transform(UrlTransform::RemoveComponents {
            components: vec![UrlComponent::Path],
        });

        assert_eq!(
            apply(&rule, "https://example.com/a/b?id=1#frag"),
            Some("https://example.com/?id=1#frag".into())
        );
        // Already at the root: no change, so no match.
        assert_eq!(apply(&rule, "https://example.com/?id=1"), None);
        assert_eq!(apply(&rule, "https://example.com"), None);
    }

    #[test]
    fn remove_components_can_be_combined_and_is_order_independent() {
        let forward = transform(UrlTransform::RemoveComponents {
            components: vec![
                UrlComponent::Query,
                UrlComponent::Fragment,
                UrlComponent::Port,
                UrlComponent::Path,
                UrlComponent::Credentials,
            ],
        });
        let reversed = transform(UrlTransform::RemoveComponents {
            components: vec![
                UrlComponent::Credentials,
                UrlComponent::Path,
                UrlComponent::Port,
                UrlComponent::Fragment,
                UrlComponent::Query,
            ],
        });

        let input = "https://user:pw@example.com:8443/a/b?id=1#frag";
        let expected = Some("https://example.com/".to_string());
        assert_eq!(apply(&forward, input), expected);
        assert_eq!(apply(&reversed, input), expected);
    }

    #[test]
    fn structural_rules_never_touch_non_http_urls() {
        let path = transform(UrlTransform::RemoveComponents {
            components: vec![UrlComponent::Path],
        });
        let host = transform(UrlTransform::RewriteHost {
            from: None,
            to: "example.com".into(),
        });

        // `set_path("/")` would corrupt a cannot-be-a-base URL, so the scheme
        // gate in `transform_url_in_place` must reject these first.
        for value in ["mailto:someone@example.com", "data:text/plain,hi"] {
            assert_eq!(apply(&path, value), None, "{value} must stay untouched");
            assert_eq!(apply(&host, value), None, "{value} must stay untouched");
        }
    }
}
