use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::EditorConfig;

pub fn open_rule_in_editor(
    path: &Path,
    line: Option<usize>,
    configured_editor: Option<&EditorConfig>,
) -> Result<()> {
    if let Some(editor) = configured_editor.filter(|editor| !editor.command.trim().is_empty()) {
        return open_with_configured_editor(editor, path, line);
    }
    let Some(editor) = preferred_editor_command() else {
        return open_config(path);
    };
    let mut parts = shlex::split(&editor).context("editor command has unmatched quotes")?;
    if parts.is_empty() {
        bail!("editor command is empty");
    }
    let program = parts.remove(0);
    let editor_name = program_name(&program);
    let mut command = Command::new(&program);
    crate::platform::environment::configure_command(&mut command);
    command.args(parts);

    match (editor_name.as_str(), line) {
        ("vim" | "nvim" | "vi", Some(line)) => {
            command.arg(format!("+{line}")).arg(path);
        }
        ("code" | "code-insiders" | "codium" | "cursor" | "windsurf", Some(line)) => {
            command
                .arg("--goto")
                .arg(format!("{}:{line}:1", path.display()));
        }
        ("zed" | "subl" | "sublime_text" | "hx" | "helix", Some(line)) => {
            command.arg(format!("{}:{line}:1", path.display()));
        }
        ("mate", Some(line)) => {
            command.arg("--line").arg(line.to_string()).arg(path);
        }
        ("emacs" | "emacsclient", Some(line)) => {
            command.arg(format!("+{line}:1")).arg(path);
        }
        ("nano", Some(line)) => {
            command.arg(format!("+{line},1")).arg(path);
        }
        ("kak" | "kakoune", Some(line)) => {
            command.arg(format!("+{line}:1")).arg(path);
        }
        (
            "idea" | "clion" | "pycharm" | "webstorm" | "rustrover" | "goland" | "rider",
            Some(line),
        ) => {
            command.arg("--line").arg(line.to_string()).arg(path);
        }
        (_, _) => {
            command.arg(path);
        }
    }

    command
        .spawn()
        .with_context(|| format!("launch editor {program}"))?;
    Ok(())
}

fn open_with_configured_editor(
    editor: &EditorConfig,
    path: &Path,
    line: Option<usize>,
) -> Result<()> {
    let program = editor.command.trim();
    let mut command = Command::new(program);
    crate::platform::environment::configure_command(&mut command);
    let line = line.unwrap_or(1).to_string();
    let mut has_file_placeholder = false;

    for argument in &editor.args {
        if argument == "{file}" {
            command.arg(path);
            has_file_placeholder = true;
            continue;
        }
        has_file_placeholder |= argument.contains("{file}");
        command.arg(
            argument
                .replace("{file}", &path.to_string_lossy())
                .replace("{line}", &line)
                .replace("{column}", "1"),
        );
    }
    if !has_file_placeholder {
        command.arg(path);
    }

    command
        .spawn()
        .with_context(|| format!("launch configured editor {program}"))?;
    Ok(())
}

pub fn open_config(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("/usr/bin/open");
    #[cfg(target_os = "windows")]
    let mut command = {
        use std::os::windows::process::CommandExt;

        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

        let mut command = Command::new("cmd.exe");
        command.args(["/C", "start", ""]);
        command.creation_flags(CREATE_NO_WINDOW);
        command
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = Command::new("xdg-open");

    crate::platform::environment::configure_command(&mut command);
    command
        .arg(path)
        .spawn()
        .with_context(|| format!("open config {}", path.display()))?;
    Ok(())
}

pub fn reveal_config(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("/usr/bin/open");
        command.arg("-R");
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("explorer.exe");
        command.arg(format!("/select,{}", path.display()));
        command
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = Command::new("xdg-open");

    crate::platform::environment::configure_command(&mut command);
    #[cfg(not(target_os = "windows"))]
    command.arg(if cfg!(target_os = "linux") {
        path.parent().unwrap_or(path)
    } else {
        path
    });
    command
        .spawn()
        .with_context(|| format!("reveal config {}", path.display()))?;
    Ok(())
}

fn program_name(program: &str) -> String {
    let file_name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    Path::new(file_name)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(file_name)
        .to_ascii_lowercase()
}

fn preferred_editor_command() -> Option<String> {
    let visual =
        crate::platform::environment::var("VISUAL").filter(|value| !value.trim().is_empty());

    #[cfg(target_os = "windows")]
    let editor = visual;
    #[cfg(not(target_os = "windows"))]
    let editor = visual.or_else(|| {
        crate::platform::environment::var("EDITOR").filter(|value| !value.trim().is_empty())
    });

    editor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_name_uses_the_executable_name() {
        assert_eq!(
            program_name("/Applications/Visual Studio Code.app/code"),
            "code"
        );
        assert_eq!(
            program_name(r"C:\Program Files\VSCodium\codium.exe"),
            "codium"
        );
    }

    #[test]
    fn quoted_editor_commands_keep_program_and_existing_arguments_together() {
        let parts =
            shlex::split(r#""/Applications/Visual Studio Code.app/code" --reuse-window"#).unwrap();
        assert_eq!(
            parts,
            [
                "/Applications/Visual Studio Code.app/code",
                "--reuse-window"
            ]
        );
    }
}
