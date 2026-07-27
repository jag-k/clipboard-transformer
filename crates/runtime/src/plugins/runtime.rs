//! Extism adapter for the runtime-neutral protocol in [`super::protocol`].
//!
//! This is the only module allowed to know that plugins run on Extism. The
//! payload encoding is JSON over the byte-oriented Extism call ABI. Limits
//! are host policy applied at instantiation; a plugin trap or timeout returns
//! an error from the call instead of terminating the host.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Host;

use ct_plugin_api::{
    CompileRuleRequest, CompileRuleResponse, InitializeRequest, InitializeResponse,
    TransformRequest, TransformResponse,
};

/// Exports every plugin module must provide.
pub use ct_plugin_api::REQUIRED_EXPORTS;

/// Host-owned resource limits applied to every plugin instance.
#[derive(Debug, Clone)]
pub struct PluginLimits {
    /// Maximum linear memory in 64 KiB WASM pages.
    pub memory_max_pages: u32,
    /// Wall-clock limit for a single call.
    pub call_timeout: Duration,
    /// Maximum HTTP response size delivered to the plugin.
    pub max_http_response_bytes: u64,
    /// Maximum bytes in the plugin variable store.
    pub max_var_bytes: u64,
    /// Maximum bytes accepted from a single plugin response payload.
    pub max_response_bytes: usize,
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self {
            memory_max_pages: 1024, // 64 MiB
            call_timeout: Duration::from_secs(5),
            max_http_response_bytes: 4 * 1024 * 1024,
            max_var_bytes: 1024 * 1024,
            max_response_bytes: 16 * 1024 * 1024,
        }
    }
}

/// One instantiated plugin. Calls are serialized by the owner (the rule
/// engine worker or the initialization path), never by the native UI thread.
pub struct PluginRuntime {
    plugin: extism::Plugin,
    max_response_bytes: usize,
}

#[derive(Debug, Clone)]
struct HttpHostPolicy {
    patterns: Vec<String>,
}

impl HttpHostPolicy {
    fn allows(&self, candidate: &str) -> bool {
        let Some(host) = literal_http_host(candidate) else {
            return false;
        };
        self.patterns.iter().any(|pattern| {
            glob::Pattern::new(pattern)
                .map(|pattern| pattern.matches(&host))
                .unwrap_or_else(|_| pattern == &host)
        })
    }
}

/// Treats guest input only as a concrete candidate hostname. In particular,
/// glob metacharacters are rejected instead of being interpreted as policy.
fn literal_http_host(candidate: &str) -> Option<String> {
    if candidate.is_empty()
        || candidate != candidate.trim()
        || candidate.contains(['*', '?', '[', ']'])
    {
        return None;
    }
    let host = Host::parse(candidate).ok()?.to_string();
    (!host.contains(['*', '?', '[', ']'])).then_some(host)
}

extism::host_fn!(http_host_allowed(policy: HttpHostPolicy; host: String) -> extism::convert::Json<bool> {
    let policy = policy.get()?;
    let policy = policy
        .lock()
        .map_err(|_| anyhow!("HTTP host policy mutex is poisoned"))?;
    Ok(extism::convert::Json(policy.allows(&host)))
});

impl PluginRuntime {
    /// Instantiates a module with the granted capabilities and host limits.
    ///
    /// HTTP host patterns are passed directly to Extism, which applies its
    /// own glob matcher to the hostname of each outbound request.
    pub fn load(wasm: &[u8], http_hosts: &[String], limits: &PluginLimits) -> Result<Self> {
        let manifest = extism::Manifest::new([extism::Wasm::data(wasm.to_vec())])
            .with_memory_options(
                extism_manifest::MemoryOptions::new()
                    .with_max_pages(limits.memory_max_pages)
                    .with_max_http_response_bytes(limits.max_http_response_bytes)
                    .with_max_var_bytes(limits.max_var_bytes),
            )
            .with_timeout(limits.call_timeout)
            .with_allowed_hosts(http_hosts.iter().cloned());
        // Keep the XTP Rust template's WASI target working. No filesystem
        // directories are preopened because the manifest has no allowed paths.
        let plugin = extism::PluginBuilder::new(&manifest)
            .with_wasi(true)
            .with_function(
                "http_host_allowed",
                [extism::ValType::I64],
                [extism::ValType::I64],
                extism::UserData::new(HttpHostPolicy {
                    patterns: http_hosts.to_vec(),
                }),
                http_host_allowed,
            )
            .build()
            .context("instantiate plugin module")?;
        for export in REQUIRED_EXPORTS {
            if !plugin.function_exists(export) {
                bail!("plugin module does not export required function {export:?}");
            }
        }
        Ok(Self {
            plugin,
            max_response_bytes: limits.max_response_bytes,
        })
    }

    pub fn initialize(&mut self, request: &InitializeRequest) -> Result<InitializeResponse> {
        self.call_json("initialize", request)
    }

    pub fn compile_rule(&mut self, request: &CompileRuleRequest) -> Result<CompileRuleResponse> {
        self.call_json("compile_rule", request)
    }

    pub fn transform(&mut self, request: &TransformRequest) -> Result<TransformResponse> {
        self.call_json("transform", request)
    }

    fn call_json<Request: Serialize, Response: DeserializeOwned>(
        &mut self,
        name: &str,
        request: &Request,
    ) -> Result<Response> {
        let input = serde_json::to_vec(request).context("serialize plugin request")?;
        let output: Vec<u8> = self
            .plugin
            .call(name, input)
            .map_err(|error| anyhow!("plugin call {name} failed: {error}"))?;
        if output.len() > self.max_response_bytes {
            bail!(
                "plugin call {name} returned {} bytes; the limit is {}",
                output.len(),
                self.max_response_bytes
            );
        }
        serde_json::from_slice(&output)
            .with_context(|| format!("parse plugin response from {name}"))
    }
}

impl std::fmt::Debug for PluginRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginRuntime")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_policy_matches_only_host_owned_globs() {
        let policy = HttpHostPolicy {
            patterns: vec!["*.example.com".to_string()],
        };
        assert!(policy.allows("gitlab.example.com"));
        assert!(!policy.allows("example.com"));
        assert!(!policy.allows("other.test"));
    }

    #[test]
    fn guest_candidate_cannot_be_a_glob_or_url() {
        let policy = HttpHostPolicy {
            patterns: vec!["*".to_string()],
        };
        assert!(policy.allows("gitlab.example.com"));
        assert!(!policy.allows("*"));
        assert!(!policy.allows("*.example.com"));
        assert!(!policy.allows("%2A.example.com"));
        assert!(!policy.allows("https://gitlab.example.com"));
        assert!(!policy.allows("gitlab.example.com/path"));
    }
}
