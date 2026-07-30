//! This runtime's worker adapter around the portable transformation core.
//!
//! Only the worker lives here. The rule model and engine belong to `ct-core`,
//! the clipboard item model to `ct-clipboard`, and callers import them from
//! there. Re-exporting them through this module would give every one of those
//! types two names, and `use` blocks that already mention both crates would
//! drift between the two spellings.

pub mod shell;
mod worker;

use std::sync::Arc;

use ct_core::ExternalRuleProvider;

pub use worker::{RuleWorker, RuleWorkerCompletion, RuleWorkerOutcome, WakeSink};

pub fn external_providers(
    config: &crate::config::AppConfig,
    paths: shell::ShellHostPaths,
    rule_sources: &std::collections::BTreeMap<String, crate::config::RuleSource>,
    additional: &[Arc<dyn ExternalRuleProvider>],
) -> Vec<Arc<dyn ExternalRuleProvider>> {
    let mut providers = shell::providers(config, paths, rule_sources);
    providers.extend(additional.iter().cloned());
    providers
}
