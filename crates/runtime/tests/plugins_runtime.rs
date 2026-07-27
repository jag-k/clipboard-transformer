//! End-to-end plugin runtime tests against the XTP-generated GitLab example.
//!
//! These tests need the compiled guest module. Build it first with
//! `just build-example-plugin` (or run `just test-plugins`); without the
//! artifact tests skip with a notice so plain `cargo test` stays runnable
//! without the wasm32 toolchain target.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ct_clipboard::ClipboardItem;
use ct_core::RuleEngine;
use ct_runtime::config::{load_config_with_options, ConfigLoadOptions, ConfigWarning};
use ct_runtime::plugins::{PluginCatalog, PluginLimits, PluginState};

fn example_plugin_module() -> Option<Vec<u8>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/gitlab-link/target/wasm32-wasip1/release/gitlab_link.wasm");
    match std::fs::read(&path) {
        Ok(module) => Some(module),
        Err(_) => {
            eprintln!(
                "skipping plugin runtime test: {} not found; run `just build-example-plugin`",
                path.display()
            );
            None
        }
    }
}

fn write_plugin_dir(module: &[u8]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("gitlab-link.wasm"), module).unwrap();
    dir
}

#[test]
#[ignore = "manual RSS probe; run with `just probe-plugin-reload-memory`"]
fn repeated_plugin_replacement_reports_current_rss() {
    let Some(module) = example_plugin_module() else {
        panic!("example plugin artifact is required for the memory probe");
    };
    let dir = write_plugin_dir(&module);
    let iterations = std::env::var("PLUGIN_RELOAD_PROBE_ITERATIONS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(20);
    let mut active = None;

    eprintln!("iteration,rss_kib");
    for iteration in 0..iterations {
        // Construct the replacement before dropping the active set. This
        // intentionally models the brief old/new runtime overlap on reload.
        let replacement = PluginCatalog::discover(dir.path())
            .initialize(&BTreeMap::new(), &PluginLimits::default());
        assert_eq!(replacement.statuses()[0].state, PluginState::Operational);
        active = Some(replacement);
        eprintln!("{iteration},{}", current_rss_kib());
    }
    drop(active);
}

fn current_rss_kib() -> u64 {
    #[cfg(target_os = "macos")]
    {
        let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::zeroed();
        // SAFETY: proc_pidinfo writes at most the supplied proc_taskinfo size
        // into a valid, suitably aligned output pointer for this process.
        let written = unsafe {
            libc::proc_pidinfo(
                std::process::id() as libc::c_int,
                libc::PROC_PIDTASKINFO,
                0,
                info.as_mut_ptr().cast(),
                std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int,
            )
        };
        assert_eq!(written as usize, std::mem::size_of::<libc::proc_taskinfo>());
        // SAFETY: the size check above proves proc_pidinfo initialized it.
        unsafe { info.assume_init() }.pti_resident_size / 1024
    }

    #[cfg(target_os = "linux")]
    {
        let statm = std::fs::read_to_string("/proc/self/statm").expect("read /proc/self/statm");
        let resident_pages = statm
            .split_whitespace()
            .nth(1)
            .expect("statm resident pages")
            .parse::<u64>()
            .expect("statm resident pages are numeric");
        // SAFETY: _SC_PAGESIZE is a read-only process query.
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        assert!(page_size > 0);
        resident_pages.saturating_mul(page_size as u64) / 1024
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        panic!("current RSS sampling is not implemented on this platform");
    }
}

#[test]
fn example_plugin_manifest_is_discovered_without_execution() {
    let Some(module) = example_plugin_module() else {
        return;
    };
    let manifest = ct_runtime::plugins::extract_manifest(&module).unwrap();
    assert_eq!(manifest.id, "dev.jag-k.gitlab");
    assert_eq!(manifest.api_version, 1);
    assert_eq!(
        manifest.rule_type_ids().collect::<Vec<_>>(),
        [
            "dev.jag-k.gitlab/project",
            "dev.jag-k.gitlab/mr",
            "dev.jag-k.gitlab/issue",
            "dev.jag-k.gitlab/milestone",
            "dev.jag-k.gitlab/pipeline",
            "dev.jag-k.gitlab/job",
            "dev.jag-k.gitlab/commit",
            "dev.jag-k.gitlab/tag",
            "dev.jag-k.gitlab/repository"
        ]
    );
    assert!(manifest.rules.iter().all(|rule| {
        rule.settings_schema
            .as_ref()
            .and_then(|schema| schema.pointer("/properties/online/default"))
            == Some(&serde_json::json!(true))
    }));
}

#[test]
fn example_plugin_initializes_and_transforms_clipboard_text() {
    let Some(module) = example_plugin_module() else {
        return;
    };
    let dir = write_plugin_dir(&module);
    let catalog = PluginCatalog::discover(dir.path());
    assert_eq!(
        catalog.known_rule_types().into_iter().collect::<Vec<_>>(),
        [
            "dev.jag-k.gitlab/commit",
            "dev.jag-k.gitlab/issue",
            "dev.jag-k.gitlab/job",
            "dev.jag-k.gitlab/milestone",
            "dev.jag-k.gitlab/mr",
            "dev.jag-k.gitlab/pipeline",
            "dev.jag-k.gitlab/project",
            "dev.jag-k.gitlab/repository",
            "dev.jag-k.gitlab/tag"
        ]
    );

    let set = catalog.initialize(&BTreeMap::new(), &PluginLimits::default());
    assert_eq!(set.statuses().len(), 1);
    assert_eq!(set.statuses()[0].state, PluginState::Operational);
    assert!(set.statuses()[0].issues.is_empty());
    assert!(!set.statuses()[0].requires_attention());

    let rule: ct_runtime::config::ConfigDocument = serde_yaml::from_str(
        r#"
rules:
  - type: dev.jag-k.gitlab/project
    id: gitlab-project
    hosts: [gitlab.example.com]
    online: false
  - type: dev.jag-k.gitlab/mr
    id: gitlab-mr
    hosts: [gitlab.example.com]
  - type: dev.jag-k.gitlab/issue
    id: gitlab-issue
    hosts: [gitlab.example.com]
    online: false
  - type: dev.jag-k.gitlab/milestone
    id: gitlab-milestone
    hosts: [gitlab.example.com]
    online: false
  - type: dev.jag-k.gitlab/pipeline
    id: gitlab-pipeline
    hosts: [gitlab.example.com]
    online: false
  - type: dev.jag-k.gitlab/job
    id: gitlab-job
    hosts: [gitlab.example.com]
    online: false
  - type: dev.jag-k.gitlab/commit
    id: gitlab-commit
    hosts: [gitlab.example.com]
    online: false
  - type: dev.jag-k.gitlab/tag
    id: gitlab-tag
    hosts: [gitlab.example.com]
    online: false
  - type: dev.jag-k.gitlab/repository
    id: gitlab-repository
    hosts: [gitlab.example.com]
    online: false
"#,
    )
    .unwrap();
    let (mut engine, skipped) =
        RuleEngine::compile_with_external(rule.rules, set.providers()).unwrap();
    assert!(skipped.is_empty(), "{skipped:?}");
    assert_eq!(engine.rule_count(), 9);

    let input = ClipboardItem::from_text(
        "https://gitlab.example.com/acme/platform/widget/-/merge_requests/123",
    );
    let result = engine.try_apply(&input).unwrap().unwrap();
    assert_eq!(
        result.after.text(),
        Some(
            "[acme/platform/widget!123](https://gitlab.example.com/acme/platform/widget/-/merge_requests/123)"
        )
    );
    assert_eq!(
        result.message.as_deref(),
        Some("GitLab merge request link converted to Markdown")
    );
    assert_eq!(result.applied_rule_ids().collect::<Vec<_>>(), ["gitlab-mr"]);

    // A non-matching host preserves the item untouched.
    let other = ClipboardItem::from_text("https://github.com/o/r/pull/1");
    assert!(engine.try_apply(&other).unwrap().is_none());

    // Issues use the # sigil.
    let issue =
        ClipboardItem::from_text("https://gitlab.example.com/acme/platform/widget/-/issues/7");
    let result = engine.try_apply(&issue).unwrap().unwrap();
    assert_eq!(
        result.after.text(),
        Some(
            "[acme/platform/widget#7](https://gitlab.example.com/acme/platform/widget/-/issues/7)"
        )
    );
    assert_eq!(
        result.applied_rule_ids().collect::<Vec<_>>(),
        ["gitlab-issue"]
    );

    let issue_comment = ClipboardItem::from_text(
        "https://gitlab.example.com/acme/platform/widget/-/issues/7?view=all#note_123",
    );
    let result = engine.try_apply(&issue_comment).unwrap().unwrap();
    assert_eq!(
        result.after.text(),
        Some(
            "[acme/platform/widget#7 (comment)](https://gitlab.example.com/acme/platform/widget/-/issues/7?view=all#note_123)"
        )
    );

    let comment = ClipboardItem::from_text(
        "https://gitlab.example.com/acme/platform/widget/-/merge_requests/8?view=parallel#note_987654",
    );
    let result = engine.try_apply(&comment).unwrap().unwrap();
    assert_eq!(
        result.after.text(),
        Some(
            "[acme/platform/widget!8 (comment)](https://gitlab.example.com/acme/platform/widget/-/merge_requests/8?view=parallel#note_987654)"
        )
    );

    let project = ClipboardItem::from_text(
        "https://gitlab.example.com/acme/platform/widget?ref_type=heads#overview",
    );
    let result = engine.try_apply(&project).unwrap().unwrap();
    assert_eq!(
        result.after.text(),
        Some(
            "[acme/platform/widget](https://gitlab.example.com/acme/platform/widget?ref_type=heads#overview)"
        )
    );

    // Every default project-page alias supports subgroups and retains the
    // complete copied URL, including query and fragment. Nested alias paths
    // use the same exact-match behavior.
    for (page, alias) in [
        ("activity", "Activity"),
        ("analytics", "Analytics"),
        ("boards", "Issue Boards"),
        ("branches", "Branches"),
        ("ci/lint", "CI Lint"),
        ("container_registry", "Container Registry"),
        ("deployments", "Deployments"),
        ("environments", "Environments"),
        ("feature_flags", "Feature Flags"),
        ("forks", "Forks"),
        ("infrastructure", "Infrastructure"),
        ("issues", "Issues"),
        ("jobs", "Jobs"),
        ("labels", "Labels"),
        ("merge_requests", "MRs"),
        ("milestones", "Milestones"),
        ("packages", "Packages"),
        ("pipeline_schedules", "Pipeline Schedules"),
        ("pipelines", "Pipelines"),
        ("project_members", "Members"),
        ("releases", "Releases"),
        ("security/dashboard", "Security Dashboard"),
        ("snippets", "Snippets"),
        ("tags", "Tags"),
        ("wikis/home", "Wiki"),
    ] {
        let url =
            format!("https://gitlab.example.com/acme/platform/widget/-/{page}?scope=all#section");
        let result = engine
            .try_apply(&ClipboardItem::from_text(url.clone()))
            .unwrap()
            .unwrap();
        let expected = format!("[acme/platform/widget ({alias})]({url})");
        assert_eq!(
            result.after.text(),
            Some(expected.as_str()),
            "default alias {page:?}"
        );
    }

    let pipeline = ClipboardItem::from_text(
        "https://gitlab.example.com/acme/platform/widget/-/pipelines/987?ref=main#jobs",
    );
    let result = engine.try_apply(&pipeline).unwrap().unwrap();
    assert_eq!(
        result.after.text(),
        Some(
            "[acme/platform/widget Pipeline #987](https://gitlab.example.com/acme/platform/widget/-/pipelines/987?ref=main#jobs)"
        )
    );
    assert_eq!(
        result.applied_rule_ids().collect::<Vec<_>>(),
        ["gitlab-pipeline"]
    );

    let milestone = ClipboardItem::from_text(
        "https://gitlab.example.com/acme/platform/widget/-/milestones/42?tab=issues#progress",
    );
    let result = engine.try_apply(&milestone).unwrap().unwrap();
    assert_eq!(
        result.after.text(),
        Some(
            "[acme/platform/widget%42](https://gitlab.example.com/acme/platform/widget/-/milestones/42?tab=issues#progress)"
        )
    );
    assert_eq!(
        result.applied_rule_ids().collect::<Vec<_>>(),
        ["gitlab-milestone"]
    );

    let job = ClipboardItem::from_text(
        "https://gitlab.example.com/acme/platform/widget/-/jobs/654?show_trace=true#L120",
    );
    let result = engine.try_apply(&job).unwrap().unwrap();
    assert_eq!(
        result.after.text(),
        Some(
            "[acme/platform/widget Job #654](https://gitlab.example.com/acme/platform/widget/-/jobs/654?show_trace=true#L120)"
        )
    );
    assert_eq!(
        result.applied_rule_ids().collect::<Vec<_>>(),
        ["gitlab-job"]
    );

    let commit = ClipboardItem::from_text(
        "https://gitlab.example.com/acme/platform/widget/-/commit/0123456789abcdef0123456789abcdef01234567?view=parallel#diff-content",
    );
    let result = engine.try_apply(&commit).unwrap().unwrap();
    assert_eq!(
        result.after.text(),
        Some(
            "[acme/platform/widget@01234567](https://gitlab.example.com/acme/platform/widget/-/commit/0123456789abcdef0123456789abcdef01234567?view=parallel#diff-content)"
        )
    );
    assert_eq!(
        result.applied_rule_ids().collect::<Vec<_>>(),
        ["gitlab-commit"]
    );

    let tag = ClipboardItem::from_text(
        "https://gitlab.example.com/acme/platform/widget/-/tags/v2.1.0?sort=updated_desc#release",
    );
    let result = engine.try_apply(&tag).unwrap().unwrap();
    assert_eq!(
        result.after.text(),
        Some(
            "[acme/platform/widget@v2.1.0 (Tag)](https://gitlab.example.com/acme/platform/widget/-/tags/v2.1.0?sort=updated_desc#release)"
        )
    );
    assert_eq!(
        result.applied_rule_ids().collect::<Vec<_>>(),
        ["gitlab-tag"]
    );

    // Repository locators intentionally retain the complete path after the
    // page kind. A branch can contain slashes, so splitting ref from file path
    // based on URL segments would be unreliable.
    for (kind, locator, expected_location) in [
        ("tree", "feature/topic/src", "Tree: feature/topic/src"),
        (
            "blob",
            "feature/topic/src/lib.rs",
            "File: feature/topic/src/lib.rs, lines 10–20",
        ),
        (
            "raw",
            "main/assets/logo.svg",
            "Raw file: main/assets/logo.svg, lines 10–20",
        ),
        (
            "blame",
            "main/src/lib.rs",
            "Blame: main/src/lib.rs, lines 10–20",
        ),
        ("commits", "feature/topic", "Commits: feature/topic"),
        (
            "compare",
            "main...feature/topic",
            "Compare: main...feature/topic",
        ),
    ] {
        let url = format!(
            "https://gitlab.example.com/acme/platform/widget/-/{kind}/{locator}?ref_type=heads#L10-20"
        );
        let result = engine
            .try_apply(&ClipboardItem::from_text(url.clone()))
            .unwrap()
            .unwrap();
        let expected = format!("[acme/platform/widget ({expected_location})]({url})");
        assert_eq!(
            result.after.text(),
            Some(expected.as_str()),
            "repository kind {kind:?}"
        );
        assert_eq!(
            result.applied_rule_ids().collect::<Vec<_>>(),
            ["gitlab-repository"]
        );
    }

    let commit_file_url =
        "https://gitlab.example.com/acme/platform/widget/-/blob/0123456789abcdef/src/lib.rs#L3";
    let result = engine
        .try_apply(&ClipboardItem::from_text(commit_file_url))
        .unwrap()
        .unwrap();
    assert_eq!(
        result.after.text(),
        Some(
            "[acme/platform/widget (File: src/lib.rs, line 3, commit 0123456789abcdef)](https://gitlab.example.com/acme/platform/widget/-/blob/0123456789abcdef/src/lib.rs#L3)"
        )
    );

    // A slash-bearing branch cannot be separated from the file path without
    // asking GitLab which prefix is a real ref. Offline mode therefore keeps
    // the complete locator instead of inventing the wrong boundary.
    let complex_ref_url = "https://gitlab.example.com/acme/platform/widget/-/blob/feature/search-v2/docker/api.Dockerfile?ref_type=heads";
    let result = engine
        .try_apply(&ClipboardItem::from_text(complex_ref_url))
        .unwrap()
        .unwrap();
    assert_eq!(
        result.after.text(),
        Some(
            "[acme/platform/widget (File: feature/search-v2/docker/api.Dockerfile)](https://gitlab.example.com/acme/platform/widget/-/blob/feature/search-v2/docker/api.Dockerfile?ref_type=heads)"
        )
    );

    let unconfigured =
        ClipboardItem::from_text("https://gitlab.example.com/acme/platform/widget/-/unknown-area");
    assert!(engine.try_apply(&unconfigured).unwrap().is_none());

    let custom: ct_runtime::config::ConfigDocument = serde_yaml::from_str(
        r#"
rules:
  - type: dev.jag-k.gitlab/project
    id: gitlab-project-custom
    hosts: [gitlab.example.com]
    online: false
    aliases:
      analytics/code_review: Code Review Analytics
"#,
    )
    .unwrap();
    let (mut custom_engine, skipped) =
        RuleEngine::compile_with_external(custom.rules, set.providers()).unwrap();
    assert!(skipped.is_empty(), "{skipped:?}");
    let custom_url =
        "https://gitlab.example.com/acme/platform/widget/-/analytics/code_review?days=30#authors";
    let result = custom_engine
        .try_apply(&ClipboardItem::from_text(custom_url))
        .unwrap()
        .unwrap();
    assert_eq!(
        result.after.text(),
        Some(
            "[acme/platform/widget (Code Review Analytics)](https://gitlab.example.com/acme/platform/widget/-/analytics/code_review?days=30#authors)"
        )
    );

    // Supplying aliases replaces, rather than merges with, the defaults.
    let default_page =
        ClipboardItem::from_text("https://gitlab.example.com/acme/platform/widget/-/pipelines");
    assert!(custom_engine.try_apply(&default_page).unwrap().is_none());

    for (mode, expected) in [
        ("hidden", "[acme/platform/widget!8]"),
        ("marker", "[acme/platform/widget!8 (comment)]"),
        ("id", "[acme/platform/widget!8 (comment 321)]"),
        ("author", "[acme/platform/widget!8 (comment 321)]"),
        ("author-and-id", "[acme/platform/widget!8 (comment 321)]"),
    ] {
        let yaml = format!(
            r#"
rules:
  - type: dev.jag-k.gitlab/mr
    id: comment-{mode}
    hosts: [gitlab.example.com]
    online: false
    comment_display: {mode}
"#
        );
        let document: ct_runtime::config::ConfigDocument = serde_yaml::from_str(&yaml).unwrap();
        let (mut comment_engine, skipped) =
            RuleEngine::compile_with_external(document.rules, set.providers()).unwrap();
        assert!(skipped.is_empty(), "{skipped:?}");
        let url = "https://gitlab.example.com/acme/platform/widget/-/merge_requests/8#note_321";
        let result = comment_engine
            .try_apply(&ClipboardItem::from_text(url))
            .unwrap()
            .unwrap();
        assert_eq!(
            result.after.text(),
            Some(format!("{expected}({url})").as_str()),
            "comment mode {mode:?}"
        );
    }
}

#[test]
fn unused_example_plugin_stays_available_without_a_runtime_instance() {
    let Some(module) = example_plugin_module() else {
        return;
    };
    let dir = write_plugin_dir(&module);
    let rules = vec![ct_core::RawRule {
        id: "built-in".into(),
        from: Some("cat".into()),
        to: Some("dog".into()),
        ..ct_core::RawRule::default()
    }];

    let set = PluginCatalog::discover(dir.path()).initialize_for_rules(
        &BTreeMap::new(),
        &PluginLimits::default(),
        &rules,
    );

    assert_eq!(set.statuses()[0].state, PluginState::Available);
    assert_eq!(set.statuses()[0].available_rules.len(), 9);
    assert!(set.statuses()[0].issues.is_empty());
    assert!(set.providers().is_empty());
}

#[test]
fn referenced_example_plugin_is_initialized_lazily() {
    let Some(module) = example_plugin_module() else {
        return;
    };
    let dir = write_plugin_dir(&module);
    let document: ct_runtime::config::ConfigDocument = serde_yaml::from_str(
        r#"
rules:
  - type: ruleset
    id: nested
    rules:
      - type: dev.jag-k.gitlab/mr
        id: gitlab-mr
"#,
    )
    .unwrap();

    let set = PluginCatalog::discover(dir.path()).initialize_for_rules(
        &BTreeMap::new(),
        &PluginLimits::default(),
        &document.rules,
    );

    assert_eq!(set.statuses()[0].state, PluginState::Operational);
    assert_eq!(set.providers().len(), 9);
}

#[test]
fn invalid_plugin_rule_settings_are_skipped_with_a_reason() {
    let Some(module) = example_plugin_module() else {
        return;
    };
    let dir = write_plugin_dir(&module);
    let set =
        PluginCatalog::discover(dir.path()).initialize(&BTreeMap::new(), &PluginLimits::default());

    let document: ct_runtime::config::ConfigDocument = serde_yaml::from_str(
        r#"
rules:
  - type: dev.jag-k.gitlab/mr
    id: broken
    hosts: ["bad/host"]
  - id: still-works
    from: cat
    to: dog
"#,
    )
    .unwrap();
    let (mut engine, skipped) =
        RuleEngine::compile_with_external(document.rules, set.providers()).unwrap();
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].id, "broken");
    assert!(
        skipped[0].reason.contains("invalid host"),
        "{}",
        skipped[0].reason
    );

    // The rest of the configuration still applies.
    let result = engine
        .try_apply(&ClipboardItem::from_text("cat"))
        .unwrap()
        .unwrap();
    assert_eq!(result.after.text(), Some("dog"));
}

#[test]
fn config_load_keeps_plugin_rules_only_when_the_type_is_known() {
    let Some(module) = example_plugin_module() else {
        return;
    };
    let dir = write_plugin_dir(&module);
    let catalog = PluginCatalog::discover(dir.path());

    let config_dir = tempfile::tempdir().unwrap();
    let config_path = config_dir.path().join("config.yaml");
    std::fs::write(
        &config_path,
        r#"
rules:
  - type: dev.jag-k.gitlab/mr
    id: gitlab-mr
  - type: dev.example.missing/other
    id: dropped
"#,
    )
    .unwrap();

    let loaded = load_config_with_options(
        &config_path,
        ConfigLoadOptions {
            known_rule_types: catalog.known_rule_types(),
            refresh_url_imports: false,
            ..ConfigLoadOptions::default()
        },
    )
    .unwrap();

    assert_eq!(loaded.document.rules.len(), 1);
    assert_eq!(loaded.document.rules[0].id, "gitlab-mr");
    assert!(loaded.warnings.iter().any(|warning| matches!(
        warning,
        ConfigWarning::IgnoredRuleType { kind } if kind == "dev.example.missing/other"
    )));
}

#[test]
fn declared_grants_flow_through_and_env_expansion_resolves_tokens() {
    let Some(module) = example_plugin_module() else {
        return;
    };
    let dir = write_plugin_dir(&module);

    // The example plugin requests http and env-expansion, so both grants
    // become effective and the expanded token reaches the plugin.
    let configs: BTreeMap<String, ct_runtime::plugins::PluginConfig> = serde_yaml::from_str(
        r#"
dev.jag-k.gitlab:
  permissions:
    http: ["gitlab.example.com"]
    env_expansion: true
  settings:
    instances:
      - host: gitlab.example.com
        token: ${GITLAB_TOKEN:?token is required}
"#,
    )
    .unwrap();
    let set = PluginCatalog::discover(dir.path()).initialize_with_env(
        &configs,
        &PluginLimits::default(),
        &|name| (name == "GITLAB_TOKEN").then(|| "secret".to_string()),
    );
    let status = &set.statuses()[0];
    assert_eq!(status.state, PluginState::Operational, "{status:?}");
    assert!(status.issues.is_empty(), "{:?}", status.issues);
    assert_eq!(status.granted_http_hosts, ["gitlab.example.com"]);
    assert!(status.granted.env_expansion);
}

#[test]
fn failed_required_env_expansion_blocks_the_plugin_without_running_it() {
    let Some(module) = example_plugin_module() else {
        return;
    };
    let dir = write_plugin_dir(&module);

    let configs: BTreeMap<String, ct_runtime::plugins::PluginConfig> = serde_yaml::from_str(
        r#"
dev.jag-k.gitlab:
  permissions:
    env_expansion: true
  settings:
    instances:
      - host: gitlab.example.com
        token: ${GITLAB_TOKEN:?GITLAB_TOKEN is required}
"#,
    )
    .unwrap();
    let set = PluginCatalog::discover(dir.path()).initialize_with_env(
        &configs,
        &PluginLimits::default(),
        &|_| None,
    );
    let status = &set.statuses()[0];
    assert_eq!(status.state, PluginState::Blocked);
    assert!(status
        .issues
        .iter()
        .any(|issue| issue.code == "env-expansion-failed"
            && issue.summary.contains("GITLAB_TOKEN is required")));
    assert!(set.providers().is_empty());
}

#[test]
fn ungranted_instance_reports_informational_issue_and_falls_back_offline() {
    let Some(module) = example_plugin_module() else {
        return;
    };
    let dir = write_plugin_dir(&module);

    // An instance with a token but no HTTP hosts: the plugin stays operational,
    // reports an informational issue, and Extism rejects the attempted request
    // before transform falls back to the offline label.
    let configs: BTreeMap<String, ct_runtime::plugins::PluginConfig> = serde_yaml::from_str(
        r#"
dev.jag-k.gitlab:
  settings:
    instances:
      - host: gitlab.example.com
        token: not-used
"#,
    )
    .unwrap();
    let set = PluginCatalog::discover(dir.path()).initialize(&configs, &PluginLimits::default());
    let status = &set.statuses()[0];
    assert_eq!(status.state, PluginState::Operational, "{status:?}");
    assert!(status
        .issues
        .iter()
        .any(|issue| issue.code == "instance-http-not-granted"));
    assert!(!status.requires_attention());

    // Rule hosts default to the configured instance hosts.
    let document: ct_runtime::config::ConfigDocument = serde_yaml::from_str(
        r#"
rules:
  - type: dev.jag-k.gitlab/mr
    id: gitlab-mr
"#,
    )
    .unwrap();
    let (mut engine, skipped) =
        RuleEngine::compile_with_external(document.rules, set.providers()).unwrap();
    assert!(skipped.is_empty(), "{skipped:?}");

    let input = ClipboardItem::from_text(
        "https://gitlab.example.com/acme/platform/widget/-/merge_requests/5",
    );
    let result = engine.try_apply(&input).unwrap().unwrap();
    assert_eq!(
        result.after.text(),
        Some(
            "[acme/platform/widget!5](https://gitlab.example.com/acme/platform/widget/-/merge_requests/5)"
        )
    );
}
