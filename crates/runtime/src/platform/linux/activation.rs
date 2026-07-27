use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use anyhow::{Context, Result};
use zbus::zvariant::OwnedValue;

use super::diagnostics::SUPPORT_URL;

const APPLICATION_ID: &str = "dev.jag-k.clipboard-transformer";
const APPLICATION_PATH: &str = "/dev/jag_k/clipboard_transformer";
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
    let (action_sender, action_receiver) = mpsc::channel();
    let _connection = zbus::blocking::connection::Builder::session()
        .context("connect Linux activation service to the session bus")?
        .name(APPLICATION_ID)
        .context("claim Linux application D-Bus name")?
        .serve_at(
            APPLICATION_PATH,
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
    fn activation_identity_and_action_are_fixed() {
        assert_eq!(APPLICATION_ID, "dev.jag-k.clipboard-transformer");
        assert_eq!(OPEN_SUPPORT_ACTION, "open-support");
        assert!(SUPPORT_URL.starts_with("https://"));
    }
}
