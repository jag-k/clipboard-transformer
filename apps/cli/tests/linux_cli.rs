#![cfg(target_os = "linux")]

use std::process::Command;

fn headless_command(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_clipboard-transformer"))
        .args(args)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .output()
        .unwrap()
}

#[test]
fn doctor_reports_unsupported_session_without_failing() {
    let output = headless_command(&["doctor"]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("clipboard_observation=unavailable"));
    assert!(stdout.contains("desktop_runtime_ready=false"));
    assert!(stdout.contains("desktop_blocker=clipboard-observation-unavailable"));
}

#[test]
fn watch_fails_immediately_without_polluting_stdout() {
    let output = headless_command(&["clipboard", "watch"]);

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("clipboard observation is unavailable"));
    assert!(stderr.contains("docs/linux.md"));
}
