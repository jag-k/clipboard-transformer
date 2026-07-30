use std::fs;
use std::path::PathBuf;

use ct_clipboard::ClipboardItem;
use ct_core::RuleEngine;
use ct_runtime::config::{
    collect_config_sources_best_effort, ensure_default_config, json_schema_pretty, load_config,
    load_config_with_options, load_config_with_sources, sync_config_schema_contents_next_to,
    sync_config_schema_next_to, validate_config, ConfigLoadOptions, ConfigWarning,
    CONFIG_SCHEMA_FILE_NAME, DEFAULT_CONFIG_YAML,
};
use sha2::{Digest, Sha256};

fn error_chain(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn fixture(name: &str) -> PathBuf {
    workspace_root().join("fixtures").join(name)
}

/// Workspace root from `.cargo/config.toml`, with a fallback for invocations
/// that bypass it. Do not hardcode `../..`: it breaks silently if this package
/// ever changes nesting depth.
fn workspace_root() -> PathBuf {
    if let Some(root) = std::env::var_os("CT_WORKSPACE_ROOT") {
        return PathBuf::from(root);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn url_cache_file_name(url: &str) -> String {
    let encoded = url
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    url::Url::parse(url)
        .unwrap()
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .and_then(|name| name.rsplit_once('.').map(|(_, ext)| ext))
        .map_or(encoded.clone(), |ext| format!("{encoded}.{ext}"))
}

#[test]
fn yaml_import_flattens_rules() {
    let config = load_config(fixture("config.yaml")).unwrap();
    assert_eq!(config.rules.len(), 2);
    assert_eq!(config.rules[0].id, "shorts-to-watch");
}

#[test]
fn yaml_import_sources_are_tracked() {
    let loaded = load_config_with_sources(fixture("config.yaml")).unwrap();
    let source_names = loaded
        .sources
        .iter()
        .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
        .collect::<Vec<_>>();

    assert!(source_names.contains(&"config.yaml"));
    assert!(source_names.contains(&"youtube.yaml"));
}

#[test]
fn yaml_config_parses_structured_editor_override() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(
        &path,
        r#"
config:
  editor:
    command: code
    args: ["--goto", "{file}:{line}:{column}"]
rules: []
"#,
    )
    .unwrap();

    let config = load_config(&path).unwrap();
    let editor = config.config.editor.unwrap();
    assert_eq!(editor.command, "code");
    assert_eq!(editor.args, ["--goto", "{file}:{line}:{column}"]);
}

#[test]
fn enabled_root_shell_rule_is_retained_as_a_native_external_rule() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(
        &path,
        r#"
config:
  shell:
    enabled: true
rules:
  - id: uppercase
    type: shell
    run: tr '[:lower:]' '[:upper:]'
"#,
    )
    .unwrap();

    let loaded = load_config_with_sources(&path).unwrap();
    assert_eq!(loaded.document.rules.len(), 1);
    assert_eq!(loaded.document.rules[0].kind.as_deref(), Some("shell"));
}

#[test]
fn disabled_shell_rule_is_rejected_before_engine_compilation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(
        &path,
        r#"
rules:
  - id: untrusted
    type: shell
    run: echo unsafe
"#,
    )
    .unwrap();

    let loaded = load_config_with_sources(&path).unwrap();
    assert!(loaded.document.rules.is_empty());
    assert!(loaded.warnings.iter().any(|warning| matches!(
        warning,
        ConfigWarning::InvalidRule {
            id: Some(id),
            kind,
            reason,
        } if id == "untrusted" && kind == "shell" && reason.contains("disabled")
    )));
}

#[test]
fn local_import_shell_rule_obeys_local_import_policy() {
    let dir = tempfile::tempdir().unwrap();
    let imported = dir.path().join("rules.yaml");
    fs::write(
        &imported,
        r#"
- id: imported-shell
  type: shell
  run: tr a-z A-Z
"#,
    )
    .unwrap();
    let root = dir.path().join("config.yaml");
    fs::write(
        &root,
        r#"
config:
  shell:
    enabled: true
    local_imports: false
rules:
  - import: rules.yaml
"#,
    )
    .unwrap();

    let loaded = load_config_with_sources(&root).unwrap();
    assert!(loaded.document.rules.is_empty());
    assert!(loaded.warnings.iter().any(|warning| matches!(
        warning,
        ConfigWarning::InvalidRule {
            id: Some(id),
            reason,
            ..
        } if id == "imported-shell" && reason.contains("local imports")
    )));
}

#[test]
fn pinned_remote_import_can_explicitly_authorize_shell_rules() {
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");
    let cache_dir = state_dir.join("url-imports");
    fs::create_dir_all(&cache_dir).unwrap();
    let url = "https://example.com/trusted-shell.yaml";
    let imported = r#"
rules:
  - id: remote-shell
    type: shell
    run: tr a-z A-Z
"#;
    let cache_path = cache_dir.join(url_cache_file_name(url));
    fs::write(&cache_path, imported).unwrap();
    let digest = format!("{:x}", Sha256::digest(imported.as_bytes()));
    let root = dir.path().join("config.yaml");
    fs::write(
        &root,
        format!(
            r#"
config:
  import_refresh_interval: 0
  shell:
    enabled: true
    remote_imports: true
rules:
  - import:
      source: {url}
      permissions:
        shell: true
      sha256: {digest}
"#
        ),
    )
    .unwrap();

    let loaded = load_config_with_options(
        &root,
        ConfigLoadOptions {
            state_dir: Some(state_dir),
            refresh_url_imports: false,
            ..ConfigLoadOptions::default()
        },
    )
    .unwrap();
    assert_eq!(loaded.document.rules.len(), 1);
    assert_eq!(loaded.document.rules[0].id, "remote-shell");
}

#[test]
fn remote_shell_import_rejects_a_mismatched_pin() {
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");
    let cache_dir = state_dir.join("url-imports");
    fs::create_dir_all(&cache_dir).unwrap();
    let url = "https://example.com/untrusted-shell.yaml";
    let cache_path = cache_dir.join(url_cache_file_name(url));
    fs::write(
        &cache_path,
        "rules:\n  - id: remote-shell\n    type: shell\n    run: echo bad\n",
    )
    .unwrap();
    let root = dir.path().join("config.yaml");
    fs::write(
        &root,
        format!(
            r#"
config:
  import_refresh_interval: 0
  shell:
    enabled: true
    remote_imports: true
rules:
  - import:
      source: {url}
      permissions:
        shell: true
      sha256: {}
"#,
            "0".repeat(64)
        ),
    )
    .unwrap();

    let error = load_config_with_options(
        &root,
        ConfigLoadOptions {
            state_dir: Some(state_dir),
            refresh_url_imports: false,
            ..ConfigLoadOptions::default()
        },
    )
    .unwrap_err();
    assert!(error_chain(&error).contains("SHA-256 mismatch"));
}

#[test]
fn yaml_url_rule_parses_and_applies() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(
        &path,
        r#"
rules:
  - type: url
    id: clean-and-pin
    hosts: [example.com]
    transform:
      type: set-query-param
      name: view
      value: compact
"#,
    )
    .unwrap();

    let config = load_config(&path).unwrap();
    let mut engine = RuleEngine::compile(config.rules).unwrap();
    let result = engine
        .apply(&ClipboardItem::from_text("https://example.com/a?keep=%20"))
        .unwrap();
    assert_eq!(
        result.after.text(),
        Some("https://example.com/a?keep=%20&view=compact")
    );
}

#[test]
fn toml_url_rule_parses_and_applies() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
[[rules]]
type = "url"
id = "canonical-host"

[rules.transform]
type = "rewrite-host"
from = "www.example.com"
to = "example.com"
"#,
    )
    .unwrap();

    let config = load_config(&path).unwrap();
    let mut engine = RuleEngine::compile(config.rules).unwrap();
    let result = engine
        .apply(&ClipboardItem::from_text("https://www.example.com/a"))
        .unwrap();
    assert_eq!(result.after.text(), Some("https://example.com/a"));
}

#[test]
fn rule_sources_track_imported_and_nested_rules() {
    let loaded = load_config_with_sources(fixture("config.yaml")).unwrap();

    let imported = loaded.rule_sources.get("shorts-to-watch").unwrap();
    assert!(imported.path.ends_with("youtube.yaml"));
    assert_eq!(imported.line, 3);

    let nested = loaded.rule_sources.get("trim-protocol-example").unwrap();
    assert!(nested.path.ends_with("config.yaml"));
    assert_eq!(nested.line, 14);
}

#[test]
fn yaml_import_sources_are_tracked_best_effort_when_import_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config.yaml");
    let included = dir.path().join("bad.yaml");
    fs::write(
        &root,
        r#"
        rules:
          - import: bad.yaml
        "#,
    )
    .unwrap();
    fs::write(&included, "rules: [").unwrap();

    let sources = collect_config_sources_best_effort(&root);

    assert!(sources.iter().any(|path| path.ends_with("config.yaml")));
    assert!(sources.iter().any(|path| path.ends_with("bad.yaml")));
}

#[test]
fn yaml_config_applies_imported_rule() {
    let config = load_config(fixture("config.yaml")).unwrap();
    let mut engine = RuleEngine::compile(config.rules).unwrap();
    let result = engine
        .apply(&ClipboardItem::from_text(
            "https://www.youtube.com/shorts/abc-123",
        ))
        .unwrap();

    assert_eq!(
        result.after.text(),
        Some("https://youtube.com/watch?v=abc-123")
    );
}

#[test]
fn yaml_diamond_import_loads_shared_rules_once() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("common.yaml"),
        r#"
        rules:
          - id: cat-to-dog
            from: cat
            to: dog
        "#,
    )
    .unwrap();
    fs::write(
        dir.path().join("a.yaml"),
        "rules:\n  - import: common.yaml\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("b.yaml"),
        "rules:\n  - import: common.yaml\n",
    )
    .unwrap();
    let root = dir.path().join("config.yaml");
    fs::write(
        &root,
        r#"
        rules:
          - import: a.yaml
          - import: b.yaml
        "#,
    )
    .unwrap();

    let config = load_config(&root).unwrap();

    assert_eq!(config.rules.len(), 1);
    assert_eq!(config.rules[0].id, "cat-to-dog");
}

#[test]
fn yaml_import_supports_windows_drive_letter_path_reports_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config.yaml");
    fs::write(
        &root,
        r#"
        rules:
          - import: "C:\\rules\\missing.yaml"
        "#,
    )
    .unwrap();

    let error = load_config(&root).unwrap_err();

    // A drive-letter path must resolve as a filesystem path (missing here),
    // not fail as an unsupported URL scheme "c".
    let chain = error_chain(&error);
    assert!(
        chain.contains("does not exist"),
        "unexpected error: {chain}"
    );
    assert!(!chain.contains("unsupported import URL scheme"));
}

#[test]
fn yaml_import_supports_file_url() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config.yaml");
    let imported = dir.path().join("rules.yaml");
    fs::write(
        &imported,
        r#"
        rules:
          - id: cat-to-dog
            from: cat
            to: dog
        "#,
    )
    .unwrap();
    fs::write(
        &root,
        format!(
            r#"
            rules:
              - import: {}
            "#,
            url::Url::from_file_path(&imported).unwrap()
        ),
    )
    .unwrap();

    let config = load_config(&root).unwrap();

    assert_eq!(config.rules[0].id, "cat-to-dog");
}

#[test]
fn yaml_import_rejects_mapping_without_rules_key() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config.yaml");
    let imported = dir.path().join("rules.yaml");
    fs::write(&imported, "not_rules: []\n").unwrap();
    fs::write(
        &root,
        r#"
        rules:
          - import: rules.yaml
        "#,
    )
    .unwrap();

    let err = load_config(&root).unwrap_err();

    assert!(error_chain(&err).contains("rules key"));
}

#[test]
fn yaml_import_rejects_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config.yaml");
    fs::write(
        &root,
        r#"
        rules:
          - import: missing.yaml
        "#,
    )
    .unwrap();

    let err = load_config(&root).unwrap_err();

    assert!(error_chain(&err).contains("does not exist"));
}

#[test]
fn yaml_legacy_include_key_is_rejected_with_clear_message() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config.yaml");
    fs::write(
        &root,
        r#"
        rules:
          - include: rules.yaml
        "#,
    )
    .unwrap();

    let err = load_config(&root).unwrap_err();
    let chain = error_chain(&err);

    assert!(chain.contains("YAML `include` is unsupported"));
    assert!(chain.contains("import: path-or-url"));
}

#[test]
fn yaml_import_skips_self_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config.yaml");
    fs::write(
        &root,
        r#"
        rules:
          - import: config.yaml
          - id: local-rule
            from: cat
            to: dog
        "#,
    )
    .unwrap();

    let config = load_config(&root).unwrap();

    assert_eq!(config.rules.len(), 1);
    assert_eq!(config.rules[0].id, "local-rule");
}

#[test]
fn yaml_import_skips_transitive_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config.yaml");
    let a = dir.path().join("a.yaml");
    let b = dir.path().join("b.yaml");
    let c = dir.path().join("c.yaml");
    fs::write(
        &root,
        r#"
        rules:
          - import: a.yaml
          - id: root-rule
            from: root
            to: ok
        "#,
    )
    .unwrap();
    fs::write(
        &a,
        r#"
        rules:
          - import: b.yaml
          - id: a-rule
            from: a
            to: ok
        "#,
    )
    .unwrap();
    fs::write(
        &b,
        r#"
        rules:
          - import: c.yaml
          - id: b-rule
            from: b
            to: ok
        "#,
    )
    .unwrap();
    fs::write(
        &c,
        r#"
        rules:
          - import: a.yaml
          - id: c-rule
            from: c
            to: ok
        "#,
    )
    .unwrap();

    let config = load_config(&root).unwrap();
    let ids = config
        .rules
        .iter()
        .map(|rule| rule.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, ["c-rule", "b-rule", "a-rule", "root-rule"]);
}

#[test]
fn yaml_url_import_falls_back_to_cache_when_download_fails() {
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");
    let cache_dir = state_dir.join("url-imports");
    fs::create_dir_all(&cache_dir).unwrap();
    // Reserved TLD guarantees resolution failure for every HTTP client.
    let url = "https://unreachable.invalid/rules.yaml";
    let cache_path = cache_dir.join(url_cache_file_name(url));
    fs::write(
        &cache_path,
        r#"
        rules:
          - id: cat-to-dog
            from: cat
            to: dog
        "#,
    )
    .unwrap();
    let root = dir.path().join("config.yaml");
    fs::write(
        &root,
        format!(
            r#"
            config:
              import_refresh_interval: 1
            rules:
              - import: {url}
            "#
        ),
    )
    .unwrap();
    // Make the cache stale so a refresh is attempted (and fails).
    let stale = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(&cache_path)
        .unwrap()
        .set_modified(stale)
        .unwrap();

    let loaded = load_config_with_options(
        &root,
        ConfigLoadOptions {
            state_dir: Some(state_dir),
            refresh_url_imports: true,
            ..ConfigLoadOptions::default()
        },
    )
    .unwrap();

    assert_eq!(loaded.document.rules[0].id, "cat-to-dog");
}

#[test]
fn yaml_url_import_uses_cached_file_when_downloads_are_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");
    let cache_dir = state_dir.join("url-imports");
    fs::create_dir_all(&cache_dir).unwrap();
    let url = "https://example.com/rules.yaml";
    let cache_path = cache_dir.join(url_cache_file_name(url));
    fs::write(
        &cache_path,
        r#"
        rules:
          - id: cat-to-dog
            from: cat
            to: dog
        "#,
    )
    .unwrap();
    let root = dir.path().join("config.yaml");
    fs::write(
        &root,
        format!(
            r#"
            config:
              import_refresh_interval: 0
            rules:
              - import: {url}
            "#
        ),
    )
    .unwrap();

    let loaded = load_config_with_options(
        &root,
        ConfigLoadOptions {
            state_dir: Some(state_dir),
            refresh_url_imports: true,
            ..ConfigLoadOptions::default()
        },
    )
    .unwrap();

    assert_eq!(loaded.document.rules[0].id, "cat-to-dog");
    assert!(loaded.sources.contains(&cache_path.canonicalize().unwrap()));
    assert_eq!(
        loaded.remote_imports.get(url),
        Some(&cache_path.canonicalize().unwrap())
    );
}

#[test]
fn toml_config_applies_self_contained_rule() {
    let config = load_config(fixture("config.toml")).unwrap();
    let mut engine = RuleEngine::compile(config.rules).unwrap();
    let result = engine
        .apply(&ClipboardItem::from_text(
            "https://www.youtube.com/shorts/abc-123",
        ))
        .unwrap();

    assert_eq!(
        result.after.text(),
        Some("https://www.youtube.com/watch?v=abc-123")
    );
}

#[test]
fn toml_import_flattens_rules() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config.toml");
    let imported = dir.path().join("rules.toml");
    fs::write(
        &imported,
        r#"
        [[rules]]
        id = "cat-to-dog"
        from = "cat"
        to = "dog"
        "#,
    )
    .unwrap();
    fs::write(
        &root,
        r#"
        [[rules]]
        import = "rules.toml"

        [[rules]]
        id = "bird-to-fish"
        from = "bird"
        to = "fish"
        "#,
    )
    .unwrap();

    let loaded = load_config_with_sources(&root).unwrap();
    let ids = loaded
        .document
        .rules
        .iter()
        .map(|rule| rule.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, ["cat-to-dog", "bird-to-fish"]);
    assert!(loaded.sources.contains(&root.canonicalize().unwrap()));
    assert!(loaded.sources.contains(&imported.canonicalize().unwrap()));
}

#[test]
fn yaml_import_supports_toml_rules() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config.yaml");
    let imported = dir.path().join("rules.toml");
    fs::write(
        &imported,
        r#"
        [[rules]]
        id = "cat-to-dog"
        from = "cat"
        to = "dog"
        "#,
    )
    .unwrap();
    fs::write(
        &root,
        r#"
        rules:
          - import: rules.toml
        "#,
    )
    .unwrap();

    let config = load_config(&root).unwrap();

    assert_eq!(config.rules[0].id, "cat-to-dog");
}

#[test]
fn toml_url_import_uses_cached_file_when_downloads_are_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");
    let cache_dir = state_dir.join("url-imports");
    fs::create_dir_all(&cache_dir).unwrap();
    let url = "https://example.com/rules.toml";
    let cache_path = cache_dir.join(url_cache_file_name(url));
    fs::write(
        &cache_path,
        r#"
        [[rules]]
        id = "cat-to-dog"
        from = "cat"
        to = "dog"
        "#,
    )
    .unwrap();
    let root = dir.path().join("config.toml");
    fs::write(
        &root,
        format!(
            r#"
            [config]
            import_refresh_interval = 0

            [[rules]]
            import = "{url}"
            "#
        ),
    )
    .unwrap();

    let loaded = load_config_with_options(
        &root,
        ConfigLoadOptions {
            state_dir: Some(state_dir),
            refresh_url_imports: true,
            ..ConfigLoadOptions::default()
        },
    )
    .unwrap();

    assert_eq!(loaded.document.rules[0].id, "cat-to-dog");
    assert!(loaded.sources.contains(&cache_path.canonicalize().unwrap()));
}

#[test]
fn extensionless_local_imports_try_yaml_then_toml() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config.yaml");
    let yaml = dir.path().join("yaml-rules");
    let toml = dir.path().join("toml-rules");
    fs::write(
        &yaml,
        "config:\n  disable_for: 1\nrules:\n  - id: yaml-rule\n    from: cat\n    to: dog\n",
    )
    .unwrap();
    fs::write(
        &toml,
        "[config]\ndisable_for = 1\n\n[[rules]]\nid = \"toml-rule\"\nfrom = \"dog\"\nto = \"bird\"\n",
    )
    .unwrap();
    fs::write(
        &root,
        "config:\n  disable_for: 999\nrules:\n  - import: yaml-rules\n  - import: toml-rules\n",
    )
    .unwrap();

    let loaded = load_config_with_sources(&root).unwrap();
    let ids = loaded
        .document
        .rules
        .iter()
        .map(|rule| rule.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, ["yaml-rule", "toml-rule"]);
    assert_eq!(loaded.document.config.disable_for, 999);
    assert!(loaded.sources.contains(&yaml.canonicalize().unwrap()));
    assert!(loaded.sources.contains(&toml.canonicalize().unwrap()));
}

#[test]
fn extensionless_url_import_uses_format_detected_cache() {
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");
    let cache_dir = state_dir.join("url-imports");
    fs::create_dir_all(&cache_dir).unwrap();
    let url = "https://example.com/shared-rules";
    let cache_path = cache_dir.join(url_cache_file_name(url));
    fs::write(
        &cache_path,
        "[[rules]]\nid = \"cat-to-dog\"\nfrom = \"cat\"\nto = \"dog\"\n",
    )
    .unwrap();
    let root = dir.path().join("config.yaml");
    fs::write(
        &root,
        format!("config:\n  import_refresh_interval: 0\nrules:\n  - import: {url}\n"),
    )
    .unwrap();

    let loaded = load_config_with_options(
        &root,
        ConfigLoadOptions {
            state_dir: Some(state_dir),
            refresh_url_imports: true,
            ..ConfigLoadOptions::default()
        },
    )
    .unwrap();

    assert_eq!(loaded.document.rules[0].id, "cat-to-dog");
    assert!(loaded.sources.contains(&cache_path.canonicalize().unwrap()));
}

#[test]
fn load_reports_cycles_duplicates_and_empty_whitelists() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config.yaml");
    let nested = dir.path().join("nested.toml");
    fs::write(
        &root,
        r#"
        config:
          app_mode: whitelist
        rules:
          - import: nested.toml
          - id: duplicate
            from: cat
            to: dog
        "#,
    )
    .unwrap();
    fs::write(
        &nested,
        r#"
        [[rules]]
        import = "config.yaml"

        [[rules]]
        type = "ruleset"
        id = "group"
        mode = "all-matching"

        [[rules.rules]]
        id = "duplicate"
        from = "dog"
        to = "bird"
        app_mode = "whitelist"
        "#,
    )
    .unwrap();

    let loaded = load_config_with_sources(&root).unwrap();

    assert!(loaded.warnings.iter().any(
        |warning| matches!(warning, ConfigWarning::ImportCycle { chain } if chain.len() == 3)
    ));
    assert!(loaded.warnings.contains(&ConfigWarning::DuplicateRuleId {
        id: "duplicate".into()
    }));
    assert!(loaded
        .warnings
        .contains(&ConfigWarning::EmptyGlobalAppWhitelist));
    assert!(loaded
        .warnings
        .contains(&ConfigWarning::EmptyRuleAppWhitelist {
            id: "duplicate".into()
        }));
}

#[test]
fn unknown_plugin_rules_are_recursively_ignored_before_compile() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(
        &path,
        r#"
        rules:
          - type: future-plugin
            id: cat-to-dog
            plugin_options:
              arbitrary: [shape, 42]
              import: missing-plugin-resource.yaml
          - type: ruleset
            id: group
            rules:
              - type: nested-plugin
                id: nested-plugin-rule
                rules: {not: core-rules}
              - id: cat-to-dog
                from: cat
                to: dog
        "#,
    )
    .unwrap();

    let loaded = load_config_with_sources(&path).unwrap();
    let engine = RuleEngine::compile(loaded.document.rules.clone()).unwrap();

    assert_eq!(loaded.document.rules.len(), 1);
    assert_eq!(loaded.document.rules[0].rules.len(), 1);
    assert_eq!(engine.rule_count(), 2);
    assert!(loaded.warnings.contains(&ConfigWarning::IgnoredRuleType {
        kind: "future-plugin".into()
    }));
    assert!(loaded.warnings.contains(&ConfigWarning::IgnoredRuleType {
        kind: "nested-plugin".into()
    }));
    assert!(!loaded.warnings.contains(&ConfigWarning::DuplicateRuleId {
        id: "cat-to-dog".into()
    }));
}

#[test]
fn toml_rules_without_id_are_ignored_with_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let plugin = dir.path().join("plugin.toml");
    fs::write(
        &plugin,
        "[[rules]]\ntype = \"future-plugin\"\nopaque = { value = 1 }\n",
    )
    .unwrap();

    let loaded = load_config_with_sources(&plugin).unwrap();
    assert!(loaded.document.rules.is_empty());
    assert!(matches!(
        loaded.warnings.as_slice(),
        [ConfigWarning::InvalidRule {
            id: None,
            kind,
            reason
        }] if kind == "future-plugin" && reason.contains("id cannot be empty")
    ));

    let known = dir.path().join("known.toml");
    fs::write(
        &known,
        "[[rules]]\ntype = \"regexp\"\nfrom = \"cat\"\nto = \"dog\"\n",
    )
    .unwrap();
    let loaded = load_config_with_sources(&known).unwrap();
    assert!(loaded.document.rules.is_empty());
    assert!(matches!(
        loaded.warnings.as_slice(),
        [ConfigWarning::InvalidRule {
            id: None,
            kind,
            reason
        }] if kind == "regexp" && reason.contains("id cannot be empty")
    ));
}

#[test]
fn validation_warns_and_skips_removed_ruleset_modes() {
    for mode in ["pipeline", "full-pipeline"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        fs::write(
            &path,
            format!(
                r#"
                rules:
                  - type: ruleset
                    id: group
                    mode: {mode}
                    rules:
                      - id: cat-to-dog
                        from: cat
                        to: dog
                "#
            ),
        )
        .unwrap();

        let report = validate_config(&path, ConfigLoadOptions::default()).unwrap();
        assert!(matches!(
            report.warnings.as_slice(),
            [ConfigWarning::InvalidRule {
                id: Some(id),
                kind,
                reason
            }] if id == "group"
                && kind == "ruleset"
                && reason.contains(&format!("unknown variant `{mode}`"))
        ));
    }
}

#[test]
fn yaml_rule_without_id_is_ignored_with_warning() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(
        &path,
        r#"
        rules:
          - from: cat
            to: dog
        "#,
    )
    .unwrap();

    let loaded = load_config_with_sources(&path).unwrap();
    assert!(loaded.document.rules.is_empty());
    assert!(matches!(
        loaded.warnings.as_slice(),
        [ConfigWarning::InvalidRule {
            id: None,
            kind,
            reason
        }] if kind == "regexp" && reason.contains("id cannot be empty")
    ));
}

#[test]
fn invalid_rules_are_recursively_skipped_without_rejecting_valid_rules() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(
        &path,
        r#"
        rules:
          - id: valid
            from: cat
            to: dog
          - id: bad-regexp
            from: cat
          - type: url-cleanup
            id: bad-url
          - id: bad-app-filter
            from: dog
            to: bird
            apps: [Example]
          - type: ruleset
            id: group
            rules:
              - id: valid-nested
                from: dog
                to: fox
              - id: bad-nested
                to: wolf
        "#,
    )
    .unwrap();

    let loaded = load_config_with_sources(&path).unwrap();
    let engine = RuleEngine::compile(loaded.document.rules.clone()).unwrap();

    assert_eq!(loaded.document.rules.len(), 2);
    assert_eq!(loaded.document.rules[1].id, "group");
    assert_eq!(loaded.document.rules[1].rules.len(), 1);
    assert_eq!(engine.rule_count(), 3);
    for expected_id in ["bad-regexp", "bad-url", "bad-app-filter", "bad-nested"] {
        assert!(loaded.warnings.iter().any(|warning| matches!(
            warning,
            ConfigWarning::InvalidRule { id: Some(id), .. } if id == expected_id
        )));
    }
}

#[test]
fn yaml_unknown_fields_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(
        &path,
        r#"
        unexpected_root: true
        config:
          double_copy_window: 10
          extra_config_field: ignored
        rules:
          - id: cat-to-dog
            from: cat
            to: dog
            extra_rule_field: ignored
        "#,
    )
    .unwrap();

    let config = load_config(&path).unwrap();
    assert_eq!(config.rules[0].id, "cat-to-dog");
}

#[test]
fn toml_unknown_fields_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
        unexpected_root = true

        [config]
        double_copy_window = 10
        extra_config_field = "ignored"

        [[rules]]
        id = "cat-to-dog"
        from = "cat"
        to = "dog"
        extra_rule_field = "ignored"
        "#,
    )
    .unwrap();

    let config = load_config(&path).unwrap();
    assert_eq!(config.rules[0].id, "cat-to-dog");
}

#[test]
fn generated_schema_marks_rule_id_required() {
    let schema: serde_json::Value = serde_json::from_str(&json_schema_pretty().unwrap()).unwrap();
    let rule_schema = &schema["definitions"]["ConfigRuleSchema"];
    assert!(
        rule_schema.get("oneOf").is_some(),
        "expected oneOf rule variants"
    );
    assert!(rule_schema.get("anyOf").is_none());

    for name in [
        "RegexpRuleSchema",
        "UrlCleanupRuleSchema",
        "RulesetRuleSchema",
        "UnknownRuleSchema",
    ] {
        let required = schema["definitions"][name]["required"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} missing required"))
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();
        assert!(
            required.contains(&"id"),
            "{name} should require id, got {required:?}"
        );
        assert_eq!(
            schema["definitions"][name]["properties"]["id"]["minLength"],
            1
        );
        assert_eq!(
            schema["definitions"][name]["properties"]["id"]["pattern"],
            r"\S"
        );
    }

    assert_eq!(
        schema["definitions"]["RegexpRuleSchema"]["additionalProperties"],
        false
    );
    assert_eq!(
        schema["definitions"]["UrlCleanupRuleSchema"]["additionalProperties"],
        false
    );
    assert_eq!(
        schema["definitions"]["RulesetRuleSchema"]["additionalProperties"],
        false
    );
    assert_eq!(
        schema["definitions"]["UnknownRuleSchema"]["additionalProperties"],
        true
    );

    let regexp_type = &schema["definitions"]["RegexpRuleSchema"]["properties"]["type"];
    assert_eq!(regexp_type["enum"], serde_json::json!(["regexp"]));
    assert_eq!(regexp_type["default"], "regexp");
    assert!(
        !schema["definitions"]["RegexpRuleSchema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some("type")),
        "regexp type should remain optional so omitted type defaults work in editors"
    );
    let regexp_required = schema["definitions"]["RegexpRuleSchema"]["required"]
        .as_array()
        .unwrap();
    assert!(regexp_required
        .iter()
        .any(|value| value.as_str() == Some("from")));
    assert!(regexp_required
        .iter()
        .any(|value| value.as_str() == Some("to")));
    assert!(schema["definitions"]["RegexpRuleSchema"]["properties"]["from"].is_object());
    assert!(schema["definitions"]["RegexpRuleSchema"]["properties"]["to"].is_object());
    assert!(schema["definitions"]["RegexpRuleSchema"]["properties"]["flags"].is_object());
    assert!(schema["definitions"]["RegexpRuleSchema"]["properties"]["message"].is_object());
    assert!(
        schema["definitions"]["RegexpRuleSchema"]["properties"]["remove_query_params"].is_null()
    );
    assert!(schema["definitions"]["RegexpRuleSchema"]["properties"]["mode"].is_null());

    let url_cleanup = &schema["definitions"]["UrlCleanupRuleSchema"];
    assert_eq!(
        url_cleanup["properties"]["type"]["enum"],
        serde_json::json!(["url-cleanup"])
    );
    assert!(url_cleanup["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value.as_str() == Some("type")));
    assert!(url_cleanup["properties"]["hosts"].is_object());
    assert!(url_cleanup["properties"]["remove_query_params"].is_object());
    assert!(url_cleanup["properties"]["message"].is_object());
    assert!(url_cleanup["properties"]["from"].is_null());
    assert!(url_cleanup["properties"]["mode"].is_null());
    assert_eq!(url_cleanup["anyOf"].as_array().unwrap().len(), 3);
    assert!(url_cleanup["anyOf"]
        .as_array()
        .unwrap()
        .iter()
        .all(|branch| {
            branch["required"].as_array().is_some_and(|required| {
                required.len() == 1
                    && branch["properties"][required[0].as_str().expect("required property name")]
                        ["minItems"]
                        == 1
            })
        }));

    let url = &schema["definitions"]["UrlRuleSchema"];
    assert_eq!(
        url["properties"]["type"]["enum"],
        serde_json::json!(["url"])
    );
    assert!(url["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value.as_str() == Some("transform")));
    assert_eq!(
        url["properties"]["transform"]["allOf"][0]["$ref"],
        "#/definitions/UrlTransform"
    );

    let ruleset = &schema["definitions"]["RulesetRuleSchema"];
    assert_eq!(
        ruleset["properties"]["type"]["enum"],
        serde_json::json!(["ruleset"])
    );
    assert!(ruleset["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value.as_str() == Some("type")));
    assert!(ruleset["properties"]["mode"].is_object());
    assert_eq!(
        ruleset["properties"]["rules"]["items"]["$ref"],
        "#/definitions/ConfigRuleSchema"
    );
    assert_eq!(ruleset["properties"]["rules"]["minItems"], 1);
    assert!(ruleset["properties"]["message"].is_null());
    assert!(ruleset["properties"]["from"].is_null());
    assert!(ruleset["properties"]["remove_query_params"].is_null());

    for name in ["ShellRuleSchema", "ItemShellRuleSchema"] {
        let shell = &schema["definitions"][name];
        assert!(shell["properties"]["run"].is_object());
        assert!(shell["properties"]["script_path"].is_object());
        assert_eq!(shell["oneOf"][0]["required"], serde_json::json!(["run"]));
        assert_eq!(
            shell["oneOf"][0]["not"]["required"],
            serde_json::json!(["script_path"])
        );
        assert_eq!(
            shell["oneOf"][1]["required"],
            serde_json::json!(["script_path"])
        );
        assert_eq!(
            shell["oneOf"][1]["not"]["required"],
            serde_json::json!(["run"])
        );
    }

    let unknown_type = &schema["definitions"]["UnknownRuleSchema"]["properties"]["type"];
    assert_eq!(unknown_type["type"], "string");
    assert_eq!(
        unknown_type["not"]["enum"],
        serde_json::json!([
            "regexp",
            "url",
            "url-cleanup",
            "ruleset",
            "shell",
            "item-shell"
        ])
    );

    assert!(schema["definitions"]["ImportRuleSchema"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value.as_str() == Some("import")));
    let import = &schema["definitions"]["ImportRuleSchema"]["properties"]["import"];
    let import_description = import["description"].as_str().unwrap();
    let import_examples = import["examples"].as_array().unwrap();
    assert!(import_description.contains("GitHub files"));
    assert!(import_description.contains("Pastebin"));
    assert!(import_description.contains("parsed as YAML first, then TOML"));
    assert!(import_description.contains("config section is intentionally ignored"));
    assert_eq!(import_examples.len(), 3);
    assert_eq!(
        schema["definitions"]["RulesetMode"]["enum"],
        serde_json::json!(["all-matching", "while-matching", "all", "first"])
    );
    assert!(schema["definitions"]["RegexpRuleSchema"]["properties"]["name"].is_object());
    assert!(schema["definitions"]["RegexpRuleSchema"]["properties"]["apps"].is_object());
    assert!(schema["definitions"]["RegexpRuleSchema"]["properties"]["app_mode"].is_object());
    for name in [
        "RegexpRuleSchema",
        "UrlCleanupRuleSchema",
        "RulesetRuleSchema",
        "AppConfig",
    ] {
        let app_filter = &schema["definitions"][name]["allOf"][0];
        assert_eq!(app_filter["if"]["properties"]["apps"]["minItems"], 1);
        assert_eq!(
            app_filter["then"]["required"],
            serde_json::json!(["app_mode"])
        );
    }
    assert!(schema["definitions"]["AppConfig"]["properties"]["disable_for"].is_object());
    assert!(schema["definitions"]["AppConfig"]["properties"]["recent_items_count"].is_object());
    assert!(schema["definitions"]["AppConfig"]["properties"]["max_item_bytes"].is_object());
    assert!(schema["definitions"]["AppConfig"]["properties"]["max_history_bytes"].is_object());
    assert!(schema["definitions"]["AppConfig"]["properties"]["double_copy_window"].is_object());
    assert!(
        schema["definitions"]["AppConfig"]["properties"]["import_refresh_interval"].is_object()
    );
    assert!(schema["definitions"]["AppConfig"]["properties"]["apps"].is_object());
    assert!(schema["definitions"]["AppConfig"]["properties"]["app_mode"].is_object());
    assert!(schema["definitions"]["AppConfig"]["properties"]["disable_rule_timeout"].is_null());
    assert!(schema["definitions"]["AppConfig"]["properties"]["ignore_on_double_copy"].is_null());
}

#[test]
fn default_config_removes_tracking_query_params() {
    let config: ct_runtime::ConfigDocument = serde_yaml::from_str(DEFAULT_CONFIG_YAML).unwrap();
    let mut engine = RuleEngine::compile(config.rules).unwrap();
    let result = engine
        .apply(&ClipboardItem::from_text(
            "https://example.com/page?utm_source=news&a=1&fbclid=abc&msclkid=def#section",
        ))
        .unwrap();

    assert_eq!(
        result.after.text(),
        Some("https://example.com/page?a=1#section")
    );
}

#[test]
fn default_config_removes_tracking_query_params_by_pattern() {
    let config: ct_runtime::ConfigDocument = serde_yaml::from_str(DEFAULT_CONFIG_YAML).unwrap();
    let mut engine = RuleEngine::compile(config.rules).unwrap();
    let result = engine
        .apply(&ClipboardItem::from_text(
            "https://example.com/page?keep=1&ga_campaign=x&wtzmc=y&vn_news=z",
        ))
        .unwrap();

    assert_eq!(result.after.text(), Some("https://example.com/page?keep=1"));
}

#[test]
fn default_config_shortens_youtube_watch_without_extra_params() {
    let config: ct_runtime::ConfigDocument = serde_yaml::from_str(DEFAULT_CONFIG_YAML).unwrap();
    let mut engine = RuleEngine::compile(config.rules).unwrap();
    let result = engine
        .apply(&ClipboardItem::from_text(
            "https://www.youtube.com/watch?v=dLPAqXi9In0&utm_source=share",
        ))
        .unwrap();

    assert_eq!(result.after.text(), Some("https://youtu.be/dLPAqXi9In0"));
}

#[test]
fn default_config_keeps_youtube_playlist_urls_intact() {
    let config: ct_runtime::ConfigDocument = serde_yaml::from_str(DEFAULT_CONFIG_YAML).unwrap();
    let mut engine = RuleEngine::compile(config.rules).unwrap();

    assert!(engine
        .apply(&ClipboardItem::from_text(
            "https://www.youtube.com/watch?v=dLPAqXi9In0&list=PL123",
        ))
        .is_none());
}

#[test]
fn default_config_removes_youtu_be_share_token() {
    let config: ct_runtime::ConfigDocument = serde_yaml::from_str(DEFAULT_CONFIG_YAML).unwrap();
    let mut engine = RuleEngine::compile(config.rules).unwrap();
    let result = engine
        .apply(&ClipboardItem::from_text(
            "https://youtu.be/dLPAqXi9In0?si=fFovCtY2t-P-BAFt",
        ))
        .unwrap();

    assert_eq!(result.after.text(), Some("https://youtu.be/dLPAqXi9In0"));
}

#[test]
fn default_config_is_created_without_overwriting_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");

    assert!(ensure_default_config(&path).unwrap());
    let config_text = fs::read_to_string(&path).unwrap();
    assert!(config_text.contains("# $schema: ./clipboard-transformer.schema.json"));
    assert!(config_text.contains("remove-tracking-query-params"));
    let schema_path = dir.path().join(CONFIG_SCHEMA_FILE_NAME);
    assert!(schema_path.exists());
    let schema: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(schema_path).unwrap()).unwrap();
    assert_eq!(
        schema["properties"]["rules"]["type"].as_str(),
        Some("array")
    );

    fs::write(&path, "rules: []\n").unwrap();
    assert!(!ensure_default_config(&path).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), "rules: []\n");
}

#[test]
fn existing_config_does_not_create_missing_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(&path, "rules: []\n").unwrap();

    assert!(!ensure_default_config(&path).unwrap());

    let schema_path = dir.path().join(CONFIG_SCHEMA_FILE_NAME);
    assert!(!schema_path.exists());
}

#[test]
fn runtime_schema_sync_creates_a_missing_schema_for_an_existing_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    let schema_path = dir.path().join(CONFIG_SCHEMA_FILE_NAME);
    fs::write(&path, "rules: []\n").unwrap();

    let updated = sync_config_schema_next_to(&path).unwrap();

    assert_eq!(updated.as_deref(), Some(schema_path.as_path()));
    assert_eq!(
        fs::read_to_string(schema_path).unwrap(),
        json_schema_pretty().unwrap()
    );
}

#[test]
fn runtime_schema_sync_refreshes_an_outdated_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    let schema_path = dir.path().join(CONFIG_SCHEMA_FILE_NAME);
    fs::write(&path, "rules: []\n").unwrap();
    fs::write(&schema_path, "{}\n").unwrap();

    let updated = sync_config_schema_next_to(&path).unwrap();

    assert_eq!(updated.as_deref(), Some(schema_path.as_path()));
    assert_eq!(
        fs::read_to_string(schema_path).unwrap(),
        json_schema_pretty().unwrap()
    );
}

#[test]
fn runtime_schema_sync_accepts_precomposed_effective_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    let schema_path = dir.path().join(CONFIG_SCHEMA_FILE_NAME);
    fs::write(&path, "rules: []\n").unwrap();
    let effective_schema = "{\"plugin-rule\":true}\n";

    let updated = sync_config_schema_contents_next_to(&path, effective_schema).unwrap();

    assert_eq!(updated.as_deref(), Some(schema_path.as_path()));
    assert_eq!(fs::read_to_string(schema_path).unwrap(), effective_schema);
}

#[test]
fn yaml_plugins_section_parses_permissions_and_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(
        &path,
        r#"
plugins:
  dev.jag-k.gitlab:
    permissions:
      http:
        - gitlab.example.com
      env_expansion: true
    settings:
      token: ${GITLAB_TOKEN}
rules: []
"#,
    )
    .unwrap();

    let config = load_config(&path).unwrap();
    let plugin = config.plugins.get("dev.jag-k.gitlab").unwrap();
    assert!(plugin.permissions.env_expansion);
    assert_eq!(plugin.permissions.http, ["gitlab.example.com"]);
    assert_eq!(
        plugin
            .settings
            .get("token")
            .and_then(|value| value.as_str()),
        Some("${GITLAB_TOKEN}")
    );
}

#[test]
fn toml_plugins_section_parses_permissions_and_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
[plugins."dev.jag-k.gitlab".permissions]
env_expansion = true
http = ["gitlab.example.com"]

[plugins."dev.jag-k.gitlab".settings]
token = "${GITLAB_TOKEN}"

[[rules]]
id = "cat"
from = "cat"
to = "dog"
"#,
    )
    .unwrap();

    let config = load_config(&path).unwrap();
    let plugin = config.plugins.get("dev.jag-k.gitlab").unwrap();
    assert!(plugin.permissions.env_expansion);
    assert_eq!(plugin.permissions.http, ["gitlab.example.com"]);
    assert_eq!(config.rules.len(), 1);
}

#[test]
fn imported_toml_plugins_section_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let imported = dir.path().join("imported.toml");
    fs::write(
        &imported,
        r#"
[plugins."dev.example.ignored"]

[[rules]]
id = "imported-rule"
from = "cat"
to = "dog"
"#,
    )
    .unwrap();
    let root = dir.path().join("config.toml");
    fs::write(&root, "[[rules]]\nimport = \"imported.toml\"\n").unwrap();

    let config = load_config(&root).unwrap();
    assert_eq!(config.rules.len(), 1);
    assert!(config.plugins.is_empty());
}

#[test]
fn unknown_plugin_rule_type_is_dropped_unless_known() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(
        &path,
        r#"
rules:
  - type: dev.example.plugin/link
    id: plugin-rule
    custom_setting: value
"#,
    )
    .unwrap();

    let loaded = load_config_with_sources(&path).unwrap();
    assert!(loaded.document.rules.is_empty());
    assert!(loaded.warnings.iter().any(|warning| matches!(
        warning,
        ConfigWarning::IgnoredRuleType { kind } if kind == "dev.example.plugin/link"
    )));

    let loaded = load_config_with_options(
        &path,
        ConfigLoadOptions {
            known_rule_types: ["dev.example.plugin/link".to_string()].into(),
            refresh_url_imports: false,
            ..ConfigLoadOptions::default()
        },
    )
    .unwrap();
    assert_eq!(loaded.document.rules.len(), 1);
    let rule = &loaded.document.rules[0];
    assert_eq!(rule.id, "plugin-rule");
    assert_eq!(
        rule.plugin_settings
            .as_ref()
            .and_then(|settings| settings.get("custom_setting"))
            .and_then(|value| value.as_str()),
        Some("value")
    );
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
}

#[test]
fn imported_plugin_rules_keep_their_settings_and_sources() {
    let dir = tempfile::tempdir().unwrap();
    let imported = dir.path().join("plugin-rules.yaml");
    fs::write(
        &imported,
        r#"
rules:
  - type: dev.example.plugin/link
    id: imported-plugin-rule
    hosts: [gitlab.example.com]
"#,
    )
    .unwrap();
    let root = dir.path().join("config.yaml");
    fs::write(&root, "rules:\n  - import: plugin-rules.yaml\n").unwrap();

    let loaded = load_config_with_options(
        &root,
        ConfigLoadOptions {
            known_rule_types: ["dev.example.plugin/link".to_string()].into(),
            refresh_url_imports: false,
            ..ConfigLoadOptions::default()
        },
    )
    .unwrap();
    assert_eq!(loaded.document.rules.len(), 1);
    let rule = &loaded.document.rules[0];
    assert_eq!(rule.id, "imported-plugin-rule");
    assert_eq!(
        rule.plugin_settings
            .as_ref()
            .and_then(|settings| settings.get("hosts"))
            .and_then(|value| value.as_array())
            .map(|hosts| hosts.len()),
        Some(1)
    );
    assert!(loaded.sources.iter().any(|source| source
        .file_name()
        .is_some_and(|name| name == "plugin-rules.yaml")));
    assert!(loaded
        .rule_sources
        .get("imported-plugin-rule")
        .is_some_and(|source| source.path.ends_with("plugin-rules.yaml")));
}

#[test]
fn ruleset_with_only_plugin_children_survives_config_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(
        &path,
        r#"
rules:
  - type: ruleset
    id: plugin-pipeline
    rules:
      - type: dev.example.plugin/link
        id: nested-plugin-rule
"#,
    )
    .unwrap();

    let loaded = load_config_with_options(
        &path,
        ConfigLoadOptions {
            known_rule_types: ["dev.example.plugin/link".to_string()].into(),
            refresh_url_imports: false,
            ..ConfigLoadOptions::default()
        },
    )
    .unwrap();
    assert_eq!(loaded.document.rules.len(), 1);
    assert_eq!(loaded.document.rules[0].rules.len(), 1);
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
}

#[test]
fn imported_nested_plugin_ruleset_is_deferred_from_builtin_validation() {
    let dir = tempfile::tempdir().unwrap();
    let imported = dir.path().join("work.yaml");
    fs::write(
        &imported,
        r#"
rules:
  - type: ruleset
    id: decorated-plugin-link
    mode: all
    rules:
      - type: ruleset
        id: plugin-link-kinds
        mode: all-matching
        rules:
          - type: dev.example.plugin/project
            id: plugin-project
          - type: dev.example.plugin/issue
            id: plugin-issue
      - type: regexp
        id: decorate-plugin-link
        from: '^\[(.*)\]\((.*)\)$'
        to: '[:plugin: $1]($2)'
"#,
    )
    .unwrap();
    let root = dir.path().join("config.yaml");
    fs::write(&root, "rules:\n  - import: work.yaml\n").unwrap();
    let options = ConfigLoadOptions {
        known_rule_types: [
            "dev.example.plugin/project".to_string(),
            "dev.example.plugin/issue".to_string(),
        ]
        .into(),
        refresh_url_imports: false,
        ..ConfigLoadOptions::default()
    };

    let loaded = load_config_with_options(&root, options.clone()).unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(loaded.document.rules.len(), 1);
    let outer = &loaded.document.rules[0];
    assert_eq!(outer.id, "decorated-plugin-link");
    assert_eq!(outer.rules.len(), 2);
    assert_eq!(outer.rules[0].rules.len(), 2);

    let report = validate_config(&root, options).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn effective_schema_includes_plugin_rule_variants() {
    use ct_runtime::config::{json_schema_pretty_with_plugins, PluginRuleSchemaContribution};

    let schema = json_schema_pretty_with_plugins(&[PluginRuleSchemaContribution {
        rule_type: "dev.jag-k.gitlab/human-readable-link".to_string(),
        description: Some("GitLab links".to_string()),
        settings_schema: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "hosts": {"type": "array", "items": {"type": "string"}}
            }
        })),
    }])
    .unwrap();
    let schema: serde_json::Value = serde_json::from_str(&schema).unwrap();

    let definition = schema
        .pointer("/definitions/PluginRuleSchema_dev_jag_k_gitlab_human_readable_link")
        .expect("plugin rule definition present");
    assert_eq!(
        definition.pointer("/properties/type/enum/0"),
        Some(&serde_json::Value::String(
            "dev.jag-k.gitlab/human-readable-link".to_string()
        ))
    );
    // The unknown-type fallback excludes plugin-provided types so the oneOf
    // stays unambiguous.
    let excluded = schema
        .pointer("/definitions/UnknownRuleSchema/properties/type/not/enum")
        .and_then(serde_json::Value::as_array)
        .unwrap();
    assert!(excluded.contains(&serde_json::Value::String(
        "dev.jag-k.gitlab/human-readable-link".to_string()
    )));
    // The plugins section is part of the document schema.
    assert!(schema.pointer("/properties/plugins").is_some());
}
