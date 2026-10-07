use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use anyhow::{Context, Result};
use zbus::zvariant::OwnedValue;

use super::diagnostics::SUPPORT_URL;

/// Bus name the native packages' D-Bus service file declares.
const NATIVE_APPLICATION_ID: &str = "dev.jag-k.clipboard-transformer";
const OPEN_SUPPORT_ACTION: &str = "open-support";

struct Application {
    actions: Sender<String>,
}

#[zbus::interface(name = "org.freedesktop.Application")]
impl Application {
    #[zbus(name = "Activate")]
    fn activate(&self, _platform_data: HashMap<String, OwnedValue>) {
        crate::logging::event("ignored unexpected Linux D-Bus application activation");
    }

    #[zbus(name = "Open")]
    fn open(&self, _uris: Vec<String>, _platform_data: HashMap<String, OwnedValue>) {
        crate::logging::event("ignored unexpected Linux D-Bus open activation");
    }

    #[zbus(name = "ActivateAction")]
    fn activate_action(
        &self,
        action_name: &str,
        _parameter: Vec<OwnedValue>,
        _platform_data: HashMap<String, OwnedValue>,
    ) {
        let _ = self.actions.send(action_name.to_string());
    }
}

/// Runs the short-lived D-Bus activation path used by a failure notification
/// after the main desktop process has already exited.
pub fn run_service() -> Result<()> {
    let application_id = application_id(std::env::var("FLATPAK_ID").ok());
    let (action_sender, action_receiver) = mpsc::channel();
    let _connection = zbus::blocking::connection::Builder::session()
        .context("connect Linux activation service to the session bus")?
        .name(application_id.as_str())
        .context("claim Linux application D-Bus name")?
        .serve_at(
            object_path(&application_id),
            Application {
                actions: action_sender,
            },
        )
        .context("serve org.freedesktop.Application activation interface")?
        .build()
        .context("start Linux application activation service")?;

    let action = action_receiver
        .recv_timeout(Duration::from_secs(30))
        .context("wait for Linux notification activation")?;
    if action != OPEN_SUPPORT_ACTION {
        anyhow::bail!("unsupported Linux application action: {action}");
    }
    open_support_url()
}

/// Inside Flatpak the activation name must be the app ID: the portal and the
/// exported service file both use it, and the sandbox lets an app own its own
/// ID without an `--own-name` permission.
fn application_id(flatpak_id: Option<String>) -> String {
    flatpak_id
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| NATIVE_APPLICATION_ID.to_string())
}

/// The `org.freedesktop.Application` object path for a bus name: dots become
/// path separators and dashes, which object paths forbid, become underscores.
fn object_path(application_id: &str) -> String {
    format!("/{}", application_id.replace('.', "/").replace('-', "_"))
}

fn open_support_url() -> Result<()> {
    let portal_result: Result<()> = futures_lite::future::block_on(async {
        let proxy = ashpd::desktop::open_uri::OpenURIProxy::new()
            .await
            .context("connect to XDG Desktop Portal OpenURI interface")?;
        let uri = ashpd::Uri::parse(SUPPORT_URL).context("parse fixed Linux support URL")?;
        let request = proxy
            .open_uri(
                None,
                &uri,
                ashpd::desktop::open_uri::OpenFileOptions::default(),
            )
            .await
            .context("request opening Linux support URL")?;
        request
            .response()
            .context("open Linux support URL through portal")?;
        Ok(())
    });
    if portal_result.is_ok() {
        return Ok(());
    }
    if let Err(error) = portal_result {
        crate::logging::event(format!(
            "open support URL through portal failed, trying xdg-open: {error:#}"
        ));
    }
    std::process::Command::new("xdg-open")
        .arg(SUPPORT_URL)
        .spawn()
        .context("open fixed Linux support URL with xdg-open")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_action_is_fixed() {
        assert_eq!(OPEN_SUPPORT_ACTION, "open-support");
        assert!(SUPPORT_URL.starts_with("https://"));
    }

    #[test]
    fn native_activation_uses_the_packaged_service_name() {
        for flatpak_id in [None, Some(String::new())] {
            let id = application_id(flatpak_id);
            assert_eq!(id, "dev.jag-k.clipboard-transformer");
            assert_eq!(object_path(&id), "/dev/jag_k/clipboard_transformer");
        }
    }

    #[test]
    fn flatpak_activation_uses_the_app_id() {
        let id = application_id(Some("dev.jagk.clipboard_transformer".to_string()));
        assert_eq!(id, "dev.jagk.clipboard_transformer");
        assert_eq!(object_path(&id), "/dev/jagk/clipboard_transformer");
    }
}
