use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use directories::ProjectDirs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    /// Directory scanned for `*.wasm` plugin modules.
    pub plugins_dir: PathBuf,
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl ConfigPaths {
    pub fn resolve() -> Result<Self> {
        // Per the XDG spec, empty environment values must be treated as unset.
        let config_dir = env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join("clipboard-transformer"))
            .or_else(platform_config_dir)
            .ok_or_else(|| anyhow!("could not resolve config directory"))?;
        let state_dir = env::var_os("XDG_STATE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join("clipboard-transformer"))
            .or_else(platform_state_dir)
            .ok_or_else(|| anyhow!("could not resolve state directory"))?;
        let cache_dir = env::var_os("XDG_CACHE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join("clipboard-transformer"))
            .or_else(platform_cache_dir)
            .ok_or_else(|| anyhow!("could not resolve cache directory"))?;

        Ok(Self {
            config_file: config_dir.join("config.yaml"),
            plugins_dir: config_dir.join("plugins"),
            config_dir,
            state_dir,
            cache_dir,
        })
    }

    /// Creates the plugin discovery directory so the desktop watcher can
    /// subscribe to it before any plugins are installed.
    pub fn ensure_plugins_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.plugins_dir)
            .with_context(|| format!("create plugin directory {}", self.plugins_dir.display()))
    }
}

/// Formats a path compactly for user-facing text without changing the path
/// used for file operations.
pub fn short_path_for_display(path: &Path) -> String {
    let mut aliases = Vec::new();

    #[cfg(unix)]
    {
        push_env_alias(&mut aliases, "HOME", "~");
    }

    #[cfg(windows)]
    {
        push_env_alias(&mut aliases, "APPDATA", "%APPDATA%");
        push_env_alias(&mut aliases, "LOCALAPPDATA", "%LOCALAPPDATA%");
        push_env_alias(&mut aliases, "USERPROFILE", "%USERPROFILE%");
    }

    short_path_with_aliases(path, &aliases)
}

fn push_env_alias(aliases: &mut Vec<(PathBuf, String)>, variable: &str, replacement: &str) {
    if let Some(value) = env::var_os(variable).filter(|value| !value.is_empty()) {
        aliases.push((PathBuf::from(value), replacement.to_string()));
    }
}

fn short_path_with_aliases(path: &Path, aliases: &[(PathBuf, String)]) -> String {
    let original = path.display().to_string();
    aliases
        .iter()
        .filter_map(|(prefix, replacement)| {
            let remainder = path.strip_prefix(prefix).ok()?;
            let mut candidate = replacement.clone();
            if !remainder.as_os_str().is_empty() {
                candidate.push(std::path::MAIN_SEPARATOR);
                candidate.push_str(&remainder.display().to_string());
            }
            Some(candidate)
        })
        .min_by_key(String::len)
        .filter(|candidate| candidate.len() < original.len())
        .unwrap_or(original)
}

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("dev", "jag-k", "clipboard-transformer")
}

fn platform_config_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| {
        if cfg!(target_os = "macos") {
            // On macOS config_dir and data_dir both resolve to Application
            // Support, so keep configuration in an explicit subdirectory.
            dirs.config_dir().join("config")
        } else {
            dirs.config_dir().to_path_buf()
        }
    })
}

fn platform_state_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| {
        dirs.state_dir().map(PathBuf::from).unwrap_or_else(|| {
            if cfg!(target_os = "windows") {
                dirs.data_local_dir().join("state")
            } else {
                dirs.data_dir().join("state")
            }
        })
    })
}

fn platform_cache_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.cache_dir().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_plugins_dir_creates_the_directory_idempotently() {
        let root = tempfile::tempdir().unwrap();
        let paths = ConfigPaths {
            config_dir: root.path().to_path_buf(),
            config_file: root.path().join("config.yaml"),
            plugins_dir: root.path().join("plugins"),
            state_dir: root.path().join("state"),
            cache_dir: root.path().join("cache"),
        };

        paths.ensure_plugins_dir().unwrap();
        paths.ensure_plugins_dir().unwrap();

        assert!(paths.plugins_dir.is_dir());
    }

    #[test]
    #[cfg(unix)]
    fn short_path_uses_the_home_alias() {
        let aliases = [(PathBuf::from("/Users/example"), "~".to_string())];

        assert_eq!(
            short_path_with_aliases(
                Path::new("/Users/example/.config/clipboard-transformer/config.yaml"),
                &aliases
            ),
            "~/.config/clipboard-transformer/config.yaml"
        );
    }

    #[test]
    #[cfg(unix)]
    fn short_path_only_replaces_complete_path_components() {
        let aliases = [(PathBuf::from("/Users/example"), "~".to_string())];

        assert_eq!(
            short_path_with_aliases(Path::new("/Users/example-old/config.yaml"), &aliases),
            "/Users/example-old/config.yaml"
        );
    }

    #[test]
    #[cfg(unix)]
    fn short_path_leaves_a_shorter_absolute_path_alone() {
        let aliases = [(PathBuf::from("/"), "$VERY_LONG_ROOT_ALIAS".to_string())];

        assert_eq!(
            short_path_with_aliases(Path::new("/tmp/config.yaml"), &aliases),
            "/tmp/config.yaml"
        );
    }

    #[test]
    #[cfg(windows)]
    fn short_path_uses_a_windows_environment_alias() {
        let aliases = [(
            PathBuf::from(r"C:\Users\example\AppData\Roaming"),
            "%APPDATA%".to_string(),
        )];

        assert_eq!(
            short_path_with_aliases(
                Path::new(r"C:\Users\example\AppData\Roaming\clipboard-transformer\config.yaml"),
                &aliases
            ),
            r"%APPDATA%\clipboard-transformer\config.yaml"
        );
    }
}
