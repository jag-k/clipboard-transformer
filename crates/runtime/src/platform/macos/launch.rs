use std::path::Path;
use std::process::Command;

pub fn launched_as_app() -> bool {
    path_is_inside_app_bundle(std::env::current_exe().ok().as_deref())
        && path_is_inside_app_bundle(std::env::args_os().next().as_deref().map(Path::new))
        && !parent_is_interactive_shell()
}

fn path_is_inside_app_bundle(path: Option<&Path>) -> bool {
    path.is_some_and(|path| {
        path.components()
            .any(|component| component.as_os_str().to_string_lossy().ends_with(".app"))
    })
}

fn parent_is_interactive_shell() -> bool {
    let parent_id = std::os::unix::process::parent_id();
    Command::new("/bin/ps")
        .args(["-p", &parent_id.to_string(), "-o", "comm="])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .is_some_and(|command| {
            matches!(
                Path::new(command.trim())
                    .file_name()
                    .and_then(|name| name.to_str()),
                Some("bash" | "fish" | "nu" | "sh" | "tcsh" | "tmux" | "zsh")
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_paths_inside_an_app_bundle() {
        assert!(path_is_inside_app_bundle(Some(Path::new(
            "/Applications/Clipboard Transformer.app/Contents/MacOS/Clipboard Transformer"
        ))));
        assert!(!path_is_inside_app_bundle(Some(Path::new(
            "/usr/local/bin/clipboard-transformer"
        ))));
    }
}
