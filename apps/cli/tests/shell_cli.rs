#![cfg(unix)]

use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn transform(config: &str, input: &str) -> std::process::Output {
    let root = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_clipboard-transformer"))
        .args(["transform", "-", "--config", config])
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn transform_file(config: &Path, input: &str) -> std::process::Output {
    let root = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_clipboard-transformer"))
        .args(["transform", "-", "--config-file"])
        .arg(config)
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn example_plugin_module() -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/gitlab-link/target/wasm32-wasip1/release/gitlab_link.wasm");
    path.is_file().then_some(path)
}

#[test]
fn shell_rule_transforms_stdin_with_the_system_shell_contract() {
    let output = transform(
        r#"
config:
  shell:
    enabled: true
rules:
  - id: uppercase
    type: shell
    shell: /bin/sh
    run: tr '[:lower:]' '[:upper:]'
"#,
        "Hello shell",
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "HELLO SHELL");
}

#[test]
fn item_shell_rule_can_atomically_replace_text() {
    let output = transform(
        r#"
config:
  shell:
    enabled: true
rules:
  - id: item-output
    type: item-shell
    shell: /bin/sh
    run: |
      printf '%s' '{"action":"replace-text","text":"item result"}' > "$CT_OUTPUT_ITEM"
"#,
        "original",
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "item result");
}

#[test]
fn script_path_is_relative_to_the_declaring_import() {
    let root = tempfile::tempdir().unwrap();
    let rules_dir = root.path().join("rules");
    std::fs::create_dir_all(&rules_dir).unwrap();
    std::fs::write(
        root.path().join("config.yaml"),
        r#"
config:
  shell:
    enabled: true
rules:
  - import: rules/shell.yaml
"#,
    )
    .unwrap();
    std::fs::write(
        rules_dir.join("shell.yaml"),
        r#"
rules:
  - id: uppercase-file
    type: shell
    shell: /bin/sh
    script_path: uppercase.sh
"#,
    )
    .unwrap();
    std::fs::write(rules_dir.join("uppercase.sh"), "tr '[:lower:]' '[:upper:]'").unwrap();

    let output = transform_file(&root.path().join("config.yaml"), "Imported script");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout.clone()).unwrap(),
        "IMPORTED SCRIPT",
        "{output:?}"
    );
}

#[test]
fn plugin_transform_keeps_runtime_trace_out_of_stderr() {
    let Some(module) = example_plugin_module() else {
        eprintln!(
            "skipping plugin CLI test: run `just build-example-plugin` to build the guest module"
        );
        return;
    };
    let root = tempfile::tempdir().unwrap();
    std::fs::copy(module, root.path().join("gitlab-link.wasm")).unwrap();
    let config = r#"
rules:
  - id: gitlab-project
    type: dev.jag-k.gitlab/project
    hosts: [gitlab.example.com]
    online: false
"#;
    let mut child = Command::new(env!("CARGO_BIN_EXE_clipboard-transformer"))
        .args([
            "transform",
            "-",
            "--config",
            config,
            "--plugin-dir",
            root.path().to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"https://gitlab.example.com/acme/widget")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "[acme/widget](https://gitlab.example.com/acme/widget)"
    );
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
