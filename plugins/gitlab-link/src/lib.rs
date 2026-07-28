//! XTP-bindgen variant of the GitLab example plugin.
//!
//! `pdk.rs` is generated from `plugin-api-v1.xtp.yaml`; this file owns plugin
//! lifecycle and state, while `rules.rs` implements the rule behavior.

mod pdk;
mod rules;
mod settings;

use extism_pdk::{var, Error, Json};
use serde::{Deserialize, Serialize};

use pdk::http_host_allowed;
use pdk::types::{InitializeRequest, InitializeResponse, IssueSeverity, PluginHealth, PluginIssue};
use rules::instance_api_host;
pub(crate) use rules::{compile_rule, transform};
use settings::{Instance, PluginSettings};

const MANIFEST: &str = include_str!(concat!(env!("OUT_DIR"), "/manifest.json"));

/// Clipboard Transformer discovers this host-specific metadata without
/// executing the generated XTP/Extism exports.
#[used]
#[link_section = "clipboard-transformer/manifest"]
static MANIFEST_SECTION: [u8; MANIFEST.len()] = {
    let mut bytes = [0u8; MANIFEST.len()];
    let source = MANIFEST.as_bytes();
    let mut index = 0;
    while index < source.len() {
        bytes[index] = source[index];
        index += 1;
    }
    bytes
};

const STATE_VAR: &str = "state";

#[derive(Default, Serialize, Deserialize)]
struct StoredState {
    instances: Vec<Instance>,
}

impl StoredState {
    fn load() -> Self {
        var::get::<Json<StoredState>>(STATE_VAR)
            .ok()
            .flatten()
            .map(|Json(state)| state)
            .unwrap_or_default()
    }

    fn queryable_instance(&self, host: &str) -> Option<&Instance> {
        self.instances
            .iter()
            .find(|instance| instance.host.eq_ignore_ascii_case(host))
    }
}

pub(crate) fn initialize(request: InitializeRequest) -> Result<InitializeResponse, Error> {
    if request.api_version != 1 {
        return Ok(InitializeResponse {
            status: PluginHealth::Blocked,
            available_rules: None,
            issues: Some(vec![PluginIssue {
                attention: Some("action-required".to_string()),
                code: "unsupported-api-version".to_string(),
                details: None,
                rule_types: None,
                setting_path: None,
                severity: IssueSeverity::Error,
                summary: format!("host offered plugin API v{}", request.api_version),
            }]),
        });
    }

    let settings = match request.settings {
        None => PluginSettings::default(),
        Some(settings) => match serde_json::from_value(serde_json::Value::Object(settings)) {
            Ok(settings) => settings,
            Err(error) => {
                return Ok(InitializeResponse {
                    status: PluginHealth::Blocked,
                    available_rules: None,
                    issues: Some(vec![PluginIssue {
                        attention: Some("action-required".to_string()),
                        code: "invalid-settings".to_string(),
                        details: None,
                        rule_types: None,
                        setting_path: None,
                        severity: IssueSeverity::Error,
                        summary: format!(
                            "plugin settings do not match the expected shape: {error}"
                        ),
                    }]),
                });
            }
        },
    };

    let mut issues = Vec::new();
    for instance in &settings.instances {
        if instance.token.as_deref().unwrap_or("").is_empty() {
            issues.push(informational_issue(
                "instance-token-missing",
                format!(
                    "instance {}: no token configured; only public projects can provide titles",
                    instance.host
                ),
            ));
        }
        match instance_api_host(instance) {
            Some(host) if http_host_allowed(host.clone())? => {}
            Some(host) => issues.push(informational_issue(
                "instance-http-not-granted",
                format!(
                    "instance {}: HTTP host {host} is not granted; online titles stay disabled",
                    instance.host
                ),
            )),
            None => issues.push(informational_issue(
                "instance-api-base-invalid",
                format!(
                    "instance {}: api_base is not a valid HTTP(S) URL; online titles stay disabled",
                    instance.host
                ),
            )),
        }
    }

    var::set(
        STATE_VAR,
        Json(StoredState {
            instances: settings.instances,
        }),
    )?;
    Ok(InitializeResponse {
        status: PluginHealth::Operational,
        available_rules: None,
        issues: Some(issues),
    })
}

fn informational_issue(code: &str, summary: String) -> PluginIssue {
    PluginIssue {
        attention: Some("informational".to_string()),
        code: code.to_string(),
        details: None,
        rule_types: None,
        setting_path: None,
        severity: IssueSeverity::Warning,
        summary,
    }
}
