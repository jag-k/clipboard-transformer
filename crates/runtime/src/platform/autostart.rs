use std::path::Path;

use anyhow::{Context, Result};
#[cfg(target_os = "linux")]
use auto_launch::LinuxLaunchMode;
#[cfg(target_os = "macos")]
use auto_launch::MacOSLaunchMode;
#[cfg(target_os = "windows")]
use auto_launch::WindowsEnableMode;
use auto_launch::{AutoLaunch, AutoLaunchBuilder};

const APP_NAME: &str = "dev.jag-k.clipboard-transformer";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AutostartStatus {
    Unsupported,
    Disabled,
    Enabled,
    Error(String),
}

pub fn status() -> AutostartStatus {
    if running_in_flatpak() {
        return AutostartStatus::Unsupported;
    }
    if !AutoLaunch::is_support() {
        return AutostartStatus::Unsupported;
    }

    let result = current_launcher_executable().and_then(|executable| is_enabled(&executable));
    match result {
        Ok(true) => AutostartStatus::Enabled,
        Ok(false) => AutostartStatus::Disabled,
        Err(error) => AutostartStatus::Error(error.to_string()),
    }
}

pub fn enable(executable: &Path) -> Result<()> {
    if running_in_flatpak() {
        anyhow::bail!("autostart is unavailable inside Flatpak; use the desktop environment's startup settings");
    }
    if !AutoLaunch::is_support() {
        anyhow::bail!("autostart is unsupported on this platform");
    }

    let launcher = launcher(executable)?;
    if !launcher.is_enabled().map_err(anyhow::Error::from)? {
        launcher.enable().map_err(anyhow::Error::from)?;
    }
    Ok(())
}

pub fn enable_current() -> Result<()> {
    let executable = current_launcher_executable()?;
    enable(&executable)
}

pub fn disable() -> Result<()> {
    if running_in_flatpak() {
        anyhow::bail!("autostart is unavailable inside Flatpak; use the desktop environment's startup settings");
    }
    if !AutoLaunch::is_support() {
        anyhow::bail!("autostart is unsupported on this platform");
    }
    let executable = current_launcher_executable()?;
    let launcher = launcher(&executable)?;
    if launcher.is_enabled().map_err(anyhow::Error::from)? {
        launcher.disable().map_err(anyhow::Error::from)?;
    }
    Ok(())
}

fn current_launcher_executable() -> Result<std::path::PathBuf> {
    #[cfg(target_os = "linux")]
    if let Some(appimage) = std::env::var_os("APPIMAGE").filter(|value| !value.is_empty()) {
        let appimage = std::path::PathBuf::from(appimage);
        if appimage.is_absolute() {
            return Ok(appimage);
        }
    }
    std::env::current_exe().context("resolve current executable")
}

fn running_in_flatpak() -> bool {
    cfg!(target_os = "linux") && identifies_flatpak(std::env::var_os("FLATPAK_ID").as_deref())
}

fn identifies_flatpak(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

fn launcher(executable: &Path) -> Result<AutoLaunch> {
    let executable = executable
        .to_str()
        .with_context(|| format!("executable path is not UTF-8: {}", executable.display()))?;
    #[cfg(target_os = "windows")]
    let executable = format!("\"{executable}\"");
    #[cfg(not(target_os = "windows"))]
    let executable = executable.to_owned();
    let mut builder = AutoLaunchBuilder::new();
    builder
        .set_app_name(APP_NAME)
        .set_app_path(&executable)
        .set_args(&[] as &[&str]);
    #[cfg(target_os = "macos")]
    builder.set_macos_launch_mode(MacOSLaunchMode::SMAppService);
    #[cfg(target_os = "windows")]
    builder.set_windows_enable_mode(WindowsEnableMode::CurrentUser);
    #[cfg(target_os = "linux")]
    builder.set_linux_launch_mode(LinuxLaunchMode::XdgAutostart);
    builder.build().map_err(anyhow::Error::from)
}

fn is_enabled(executable: &Path) -> Result<bool> {
    launcher(executable)?
        .is_enabled()
        .map_err(anyhow::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_uses_the_platform_autostart_identity() {
        #[cfg(target_os = "macos")]
        let executable = Path::new(
            "/Applications/Clipboard Transformer.app/Contents/MacOS/Clipboard Transformer",
        );
        #[cfg(target_os = "windows")]
        let executable =
            Path::new(r"C:\Program Files\Clipboard Transformer\Clipboard Transformer.exe");
        #[cfg(target_os = "linux")]
        let executable = Path::new("/usr/bin/clipboard-transformer");
        let launcher = launcher(executable).unwrap();

        assert_eq!(launcher.get_app_name(), APP_NAME);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(launcher.get_app_path(), executable.to_str().unwrap());
        #[cfg(target_os = "windows")]
        assert_eq!(
            launcher.get_app_path(),
            format!("\"{}\"", executable.to_str().unwrap())
        );
        assert!(launcher.get_args().is_empty());
    }

    #[test]
    fn flatpak_requires_a_non_empty_application_id() {
        assert!(!identifies_flatpak(None));
        assert!(!identifies_flatpak(Some(std::ffi::OsStr::new(""))));
        assert!(identifies_flatpak(Some(std::ffi::OsStr::new(
            "dev.jag_k.clipboard_transformer"
        ))));
    }
}
