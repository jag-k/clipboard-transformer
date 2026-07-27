//! Build-only manifest assembly and generated guest constants.
//!
//! Plugin behavior lives in `src/rules.rs`; this build script owns descriptive
//! metadata, JSON Schemas, examples, and the merge with `manifest.base.json`.

mod settings {
    include!("src/settings.rs");
}

use settings::{
    DiscussionRuleSettings, PluginSettings, ProjectRuleSettings, RepositoryRuleSettings,
    RuleSettings,
};

struct RuleDecl {
    rust_const: &'static str,
    rule_type: &'static str,
    name: &'static str,
    description: &'static str,
    formats: &'static [&'static str],
}

const MERGE_REQUEST: RuleDecl = RuleDecl {
    rust_const: "MERGE_REQUEST",
    rule_type: "mr",
    name: "GitLab merge request",
    description: "Turns a copied GitLab merge request URL into a Markdown link; \
                  with online mode, a configured instance, and an HTTP grant, \
                  the real title is included.",
    formats: &["text", "url"],
};

const ISSUE: RuleDecl = RuleDecl {
    rust_const: "ISSUE",
    rule_type: "issue",
    name: "GitLab issue",
    description: "Turns a copied GitLab issue URL into a Markdown link; with \
                  online mode, a configured instance, and an HTTP grant, the \
                  real title is included.",
    formats: &["text", "url"],
};

const PIPELINE: RuleDecl = RuleDecl {
    rust_const: "PIPELINE",
    rule_type: "pipeline",
    name: "GitLab pipeline",
    description: "Turns a copied GitLab pipeline URL into a Markdown link; with online mode, a configured instance, and an HTTP grant, the pipeline name is included when GitLab provides one.",
    formats: &["text", "url"],
};

const MILESTONE: RuleDecl = RuleDecl {
    rust_const: "MILESTONE",
    rule_type: "milestone",
    name: "GitLab milestone",
    description: "Turns a copied GitLab project milestone URL into a Markdown link using GitLab's project%id reference; online mode includes the milestone title.",
    formats: &["text", "url"],
};

const JOB: RuleDecl = RuleDecl {
    rust_const: "JOB",
    rule_type: "job",
    name: "GitLab CI/CD job",
    description: "Turns a copied GitLab CI/CD job URL into a Markdown link; online mode includes the job name.",
    formats: &["text", "url"],
};

const COMMIT: RuleDecl = RuleDecl {
    rust_const: "COMMIT",
    rule_type: "commit",
    name: "GitLab commit",
    description: "Turns a copied GitLab commit URL into a Markdown link using GitLab's project@short-sha reference; online mode includes the commit title.",
    formats: &["text", "url"],
};

const TAG: RuleDecl = RuleDecl {
    rust_const: "TAG",
    rule_type: "tag",
    name: "GitLab repository tag",
    description: "Turns a copied GitLab repository tag URL into a Markdown link; online mode includes the tagged commit title.",
    formats: &["text", "url"],
};

const REPOSITORY: RuleDecl = RuleDecl {
    rust_const: "REPOSITORY",
    rule_type: "repository",
    name: "GitLab repository location",
    description: "Turns selected GitLab tree, blob, raw file, blame, commit-list, and comparison URLs into Markdown links without guessing where a revision ends and a file path begins.",
    formats: &["text", "url"],
};

const PROJECT: RuleDecl = RuleDecl {
    rust_const: "PROJECT",
    rule_type: "project",
    name: "GitLab project",
    description: "Turns a copied GitLab project URL into a Markdown link. Configurable aliases also match exact project pages such as merge_requests and pipelines; online mode adds the project name when available.",
    formats: &["text", "url"],
};

const RULES: &[&RuleDecl] = &[
    &PROJECT,
    &MERGE_REQUEST,
    &ISSUE,
    &MILESTONE,
    &PIPELINE,
    &JOB,
    &COMMIT,
    &TAG,
    &REPOSITORY,
];

fn main() {
    println!("cargo:rerun-if-changed=manifest.base.json");
    println!("cargo:rerun-if-changed=src/settings.rs");
    println!("cargo:rerun-if-changed=build.rs");
    let crate_dir = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"),
    );
    let base =
        std::fs::read(crate_dir.join("manifest.base.json")).expect("read manifest.base.json");
    let base = serde_json::from_slice(&base).expect("parse manifest.base.json");
    let mut assembled =
        serde_json::to_vec_pretty(&manifest_json(base)).expect("manifest serializes");
    // `to_vec_pretty` does not terminate the last line. The checked-in copy is
    // a normal text file, so without this every regeneration fights the
    // end-of-file-fixer hook.
    assembled.push(b'\n');

    let out_dir =
        std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    std::fs::write(out_dir.join("manifest.json"), &assembled)
        .expect("write generated manifest for embedding");
    std::fs::write(out_dir.join("rule_types.rs"), rule_types_rust())
        .expect("write generated rule type constants");
    let manifest = crate_dir.join("manifest.json");
    if std::fs::read(&manifest).ok().as_deref() != Some(assembled.as_slice()) {
        std::fs::write(manifest, &assembled).expect("write manifest.json");
    }
}

fn rule_types_rust() -> String {
    RULES
        .iter()
        .map(|rule| {
            format!(
                "pub const {}: &str = {:?};\n",
                rule.rust_const, rule.rule_type
            )
        })
        .collect()
}

fn manifest_json(base: serde_json::Value) -> serde_json::Value {
    let mut manifest = match base {
        serde_json::Value::Object(map) => map,
        other => panic!("manifest.base.json must be a JSON object, got {other}"),
    };
    for owned_by_code in ["rules", "settings_schema", "api_version", "version"] {
        assert!(
            !manifest.contains_key(owned_by_code),
            "manifest.base.json must not set {owned_by_code:?}; it is generated at build time"
        );
    }

    manifest.insert("api_version".into(), serde_json::json!(1));
    manifest.insert("version".into(), env!("CARGO_PKG_VERSION").into());
    manifest.insert(
        "$schema".into(),
        "https://raw.githubusercontent.com/jag-k/clipboard-transformer/main/plugins/manifest.schema.json"
            .into(),
    );
    for (field, value) in [
        ("description", env!("CARGO_PKG_DESCRIPTION")),
        ("author", env!("CARGO_PKG_AUTHORS")),
        ("license", env!("CARGO_PKG_LICENSE")),
        ("homepage", env!("CARGO_PKG_HOMEPAGE")),
    ] {
        if !value.is_empty() {
            manifest
                .entry(field)
                .or_insert_with(|| serde_json::Value::String(value.to_string()));
        }
    }

    let plugin_id = manifest
        .get("id")
        .and_then(serde_json::Value::as_str)
        .expect("manifest.base.json must set a string id")
        .to_string();
    manifest.insert(
        "settings_schema".into(),
        serde_json::to_value(schemars::schema_for!(PluginSettings)).expect("schema serializes"),
    );
    manifest.insert(
        "rules".into(),
        RULES
            .iter()
            .map(|rule| {
                serde_json::json!({
                    "type": rule.rule_type,
                    "name": rule.name,
                    "description": rule.description,
                    "settings_schema": rule_settings_schema(rule.rule_type),
                    "examples": rule_examples(&plugin_id, rule.rule_type),
                    "formats": rule.formats,
                })
            })
            .collect(),
    );
    serde_json::Value::Object(manifest)
}

fn rule_settings_schema(rule_type: &str) -> serde_json::Value {
    let schema = match rule_type {
        "project" => schemars::schema_for!(ProjectRuleSettings),
        "repository" => schemars::schema_for!(RepositoryRuleSettings),
        "mr" | "issue" => schemars::schema_for!(DiscussionRuleSettings),
        "milestone" | "pipeline" | "job" | "commit" | "tag" => {
            schemars::schema_for!(RuleSettings)
        }
        other => panic!("no settings schema registered for rule type {other:?}"),
    };
    serde_json::to_value(schema).expect("schema serializes")
}

fn rule_examples(plugin_id: &str, rule_type: &str) -> Vec<serde_json::Value> {
    match rule_type {
        "project" => vec![serde_json::json!({
            "type": format!("{plugin_id}/{rule_type}"),
            "id": "gitlab-project",
            "hosts": ["gitlab.com"],
            "online": true,
            "aliases": {
                "merge_requests": "MRs",
                "pipelines": "Pipelines",
            },
        })],
        "mr" => vec![serde_json::json!({
            "type": format!("{plugin_id}/{rule_type}"),
            "id": "gitlab-mr",
            "hosts": ["gitlab.com"],
            "online": true,
            "comment_display": "marker",
        })],
        "issue" => vec![serde_json::json!({
            "type": format!("{plugin_id}/{rule_type}"),
            "id": "gitlab-issue",
            "hosts": ["gitlab.com"],
            "online": true,
            "comment_display": "marker",
        })],
        "pipeline" => vec![serde_json::json!({
            "type": format!("{plugin_id}/{rule_type}"),
            "id": "gitlab-pipeline",
            "hosts": ["gitlab.com"],
            "online": true,
        })],
        "milestone" | "job" | "commit" | "tag" => vec![serde_json::json!({
            "type": format!("{plugin_id}/{rule_type}"),
            "id": format!("gitlab-{rule_type}"),
            "hosts": ["gitlab.com"],
            "online": true,
        })],
        "repository" => vec![serde_json::json!({
            "type": format!("{plugin_id}/{rule_type}"),
            "id": "gitlab-repository",
            "hosts": ["gitlab.com"],
            "online": true,
            "kinds": ["tree", "blob", "raw", "blame", "commits", "compare"],
        })],
        _ => Vec::new(),
    }
}
