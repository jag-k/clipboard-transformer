use std::env;
use std::ffi::OsString;
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
        let config_dir = config_dir_from(|name| env::var_os(name))
            .or_else(platform_config_dir)
            .ok_or_else(|| anyhow!("could not resolve config directory"))?;
        // Per the XDG spec, empty environment values must be treated as unset.
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

const APP_DIR: &str = "clipboard-transformer";

/// Resolves the configuration directory from the XDG environment, or `None`
/// to fall back to the platform default.
///
/// Inside Flatpak, `XDG_CONFIG_HOME` points into the per-app sandbox, which is
/// the default. A user who shares the host directory with
/// `flatpak override --filesystem=xdg-config/clipboard-transformer:create`
/// makes it visible inside the sandbox; it is then used, resolved through the
/// host's `HOST_XDG_CONFIG_HOME` or the XDG default below the shared home.
/// Until the shared directory holds a configuration of its own, an existing
/// sandbox configuration keeps being used, so granting access loses nothing.
fn config_dir_from(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    // Per the XDG spec, empty environment values must be treated as unset.
    let path = |name: &str| {
        var(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let sandboxed = path("XDG_CONFIG_HOME").map(|dir| dir.join(APP_DIR));
    if path("FLATPAK_ID").is_none() {
        return sandboxed;
    }
    let host = path("HOST_XDG_CONFIG_HOME")
        .or_else(|| path("HOME").map(|home| home.join(".config")))
        .map(|dir| dir.join(APP_DIR));
    match (host.filter(|host| host.is_dir()), sandboxed) {
        (Some(host), Some(sandboxed)) if !has_config(&host) && has_config(&sandboxed) => {
            Some(sandboxed)
        }
        (shared, sandboxed) => shared.or(sandboxed),
    }
}

fn has_config(dir: &Path) -> bool {
    ["config.yaml", "config.toml"]
        .iter()
        .any(|name| dir.join(name).is_file())
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

    fn env<'a>(vars: &'a [(&str, &Path)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.as_os_str().to_os_string())
        }
    }

    #[test]
    fn config_dir_uses_xdg_config_home_outside_flatpak() {
        let config = Path::new("/home/user/.config");
        assert_eq!(
            config_dir_from(env(&[("XDG_CONFIG_HOME", config)])),
            Some(config.join(APP_DIR))
        );
        assert_eq!(config_dir_from(env(&[])), None);
        assert_eq!(
            config_dir_from(env(&[("XDG_CONFIG_HOME", Path::new(""))])),
            None
        );
    }

    #[test]
    fn flatpak_config_dir_stays_in_the_sandbox_without_shared_access() {
        let root = tempfile::tempdir().unwrap();
        let host = root.path().join("host-config");
        let sandbox = root.path().join("sandbox-config");
        let vars = [
            ("FLATPAK_ID", Path::new("dev.jagk.clipboard_transformer")),
            ("HOST_XDG_CONFIG_HOME", host.as_path()),
            ("XDG_CONFIG_HOME", sandbox.as_path()),
            ("HOME", root.path()),
        ];

        // Without the override the host directory is not mounted at all.
        assert_eq!(config_dir_from(env(&vars)), Some(sandbox.join(APP_DIR)));
    }

    #[test]
    fn flatpak_config_dir_prefers_the_shared_host_xdg_config_home() {
        let root = tempfile::tempdir().unwrap();
        let host = root.path().join("host-config");
        let sandbox = root.path().join("sandbox-config");
        fs::create_dir_all(host.join(APP_DIR)).unwrap();
        let vars = [
            ("FLATPAK_ID", Path::new("dev.jagk.clipboard_transformer")),
            ("HOST_XDG_CONFIG_HOME", host.as_path()),
            ("XDG_CONFIG_HOME", sandbox.as_path()),
            ("HOME", root.path()),
        ];

        assert_eq!(config_dir_from(env(&vars)), Some(host.join(APP_DIR)));
    }

    #[test]
    fn flatpak_config_dir_falls_back_to_the_shared_host_home() {
        let root = tempfile::tempdir().unwrap();
        let sandbox = root.path().join("sandbox-config");
        fs::create_dir_all(root.path().join(".config").join(APP_DIR)).unwrap();
        let vars = [
            ("FLATPAK_ID", Path::new("dev.jagk.clipboard_transformer")),
            ("XDG_CONFIG_HOME", sandbox.as_path()),
            ("HOME", root.path()),
        ];

        assert_eq!(
            config_dir_from(env(&vars)),
            Some(root.path().join(".config").join(APP_DIR))
        );
    }

    #[test]
    fn flatpak_config_dir_keeps_the_sandbox_config_until_the_shared_one_exists() {
        let root = tempfile::tempdir().unwrap();
        let host = root.path().join("host-config");
        let sandbox = root.path().join("sandbox-config");
        let vars = [
            ("FLATPAK_ID", Path::new("dev.jagk.clipboard_transformer")),
            ("HOST_XDG_CONFIG_HOME", host.as_path()),
            ("XDG_CONFIG_HOME", sandbox.as_path()),
        ];
        fs::create_dir_all(sandbox.join(APP_DIR)).unwrap();
        fs::write(sandbox.join(APP_DIR).join("config.toml"), "").unwrap();
        // `:create` makes Flatpak create the shared directory before start.
        fs::create_dir_all(host.join(APP_DIR)).unwrap();

        assert_eq!(config_dir_from(env(&vars)), Some(sandbox.join(APP_DIR)));

        fs::write(host.join(APP_DIR).join("config.yaml"), "").unwrap();
        assert_eq!(config_dir_from(env(&vars)), Some(host.join(APP_DIR)));
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
