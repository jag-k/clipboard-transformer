//! Opens files from the Flatpak sandbox through the OpenURI portal.
//!
//! Inside the sandbox, editors named by `$EDITOR` or `editor.command` do not
//! exist, and the runtime's `xdg-open` forwards a read-only descriptor without
//! `writable`, so an editor that is itself a Flatpak could not save. The portal
//! takes a descriptor and hands the file to the user's host application.
//!
//! The descriptor is opened on the caller's thread so a missing file is still
//! reported to it; the portal call can wait on an application chooser, so it
//! runs on its own thread and only logs failures.

use std::fs::{File, OpenOptions};
use std::future::Future;
use std::path::Path;

use anyhow::{Context, Result};
use ashpd::desktop::open_uri::{OpenDirectoryRequest, OpenFileRequest};

/// Opens `path` in the user's default application and lets it save changes.
pub fn open_file(path: &Path) -> Result<()> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("open {} for the OpenURI portal", path.display()))?;
    in_background("open file through portal", async move {
        OpenFileRequest::default()
            .writeable(true)
            .send_file(&file)
            .await
            .context("request opening a file")?
            .response()
            .context("open a file")
    })
}

/// Shows `path` in the file manager, selecting it where the manager supports
/// it; otherwise the portal opens the containing directory.
pub fn reveal(path: &Path) -> Result<()> {
    let file = File::open(path)
        .with_context(|| format!("open {} for the OpenURI portal", path.display()))?;
    in_background("reveal path through portal", async move {
        OpenDirectoryRequest::default()
            .send(&file)
            .await
            .context("request revealing a path")?
            .response()
            .context("reveal a path")
    })
}

fn in_background(
    what: &'static str,
    request: impl Future<Output = Result<()>> + Send + 'static,
) -> Result<()> {
    std::thread::Builder::new()
        .name("portal-open".into())
        .spawn(move || {
            if let Err(error) = futures_lite::future::block_on(request) {
                crate::logging::event(format!("{what} failed: {error:#}"));
            }
        })
        .context("start portal request thread")?;
    Ok(())
}
