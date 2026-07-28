//! Bridges initialized plugins into the rule engine as
//! [`ExternalRuleProvider`] implementations.
//!
//! Compiled plugin rules run on the rule engine worker thread; the shared
//! runtime mutex serializes calls into one plugin instance when several rules
//! use it.

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};

use ct_clipboard::ClipboardSourceApp;
use ct_core::{ExternalRuleProvider, ExternalTextOutput, ExternalTextTransform, ExternalTransform};

use super::runtime::PluginRuntime;
use ct_plugin_api::{CompileRuleRequest, CompileRuleResponse, TransformRequest, TransformResponse};

pub(super) type SharedRuntime = Arc<Mutex<PluginRuntime>>;

pub(super) struct PluginRuleProvider {
    /// Full namespaced rule type, e.g. `dev.jag-k.gitlab/human-readable-link`.
    kind: String,
    /// Local rule type name inside the plugin.
    local_type: String,
    default_formats: Vec<String>,
    runtime: SharedRuntime,
}

impl PluginRuleProvider {
    pub(super) fn new(
        kind: String,
        local_type: String,
        default_formats: Vec<String>,
        runtime: SharedRuntime,
    ) -> Self {
        Self {
            kind,
            local_type,
            default_formats,
            runtime,
        }
    }
}

impl ExternalRuleProvider for PluginRuleProvider {
    fn kind(&self) -> &str {
        &self.kind
    }

    fn default_formats(&self) -> &[String] {
        &self.default_formats
    }

    fn compile(&self, _rule_id: &str, settings: &serde_json::Value) -> Result<ExternalTransform> {
        let request = CompileRuleRequest {
            rule_type: self.local_type.clone(),
            settings: settings.clone(),
        };
        let response = lock_runtime(&self.runtime)?.compile_rule(&request)?;
        match response {
            CompileRuleResponse::Ok { rule } => {
                Ok(ExternalTransform::Text(Box::new(PluginCompiledRule {
                    local_type: self.local_type.clone(),
                    rule,
                    runtime: Arc::clone(&self.runtime),
                })))
            }
            CompileRuleResponse::Error { message } => bail!("{message}"),
        }
    }
}

struct PluginCompiledRule {
    local_type: String,
    /// Opaque compiled rule value returned by the plugin's `compile_rule`.
    rule: serde_json::Value,
    runtime: SharedRuntime,
}

impl ExternalTextTransform for PluginCompiledRule {
    fn transform(
        &mut self,
        format: &str,
        value: &str,
        source_app: Option<&ClipboardSourceApp>,
    ) -> Result<Option<ExternalTextOutput>> {
        let request = TransformRequest {
            rule_type: self.local_type.clone(),
            rule: self.rule.clone(),
            format: format.to_string(),
            value: value.to_string(),
            source_app: source_app.cloned(),
        };
        let response = lock_runtime(&self.runtime)?.transform(&request)?;
        Ok(match response {
            TransformResponse::NoMatch => None,
            TransformResponse::Replace { text, message } => {
                Some(ExternalTextOutput { text, message })
            }
        })
    }
}

fn lock_runtime(runtime: &SharedRuntime) -> Result<std::sync::MutexGuard<'_, PluginRuntime>> {
    runtime
        .lock()
        .map_err(|_| anyhow!("plugin runtime mutex is poisoned"))
}
