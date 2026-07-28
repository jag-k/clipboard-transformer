//! Shared curl-based file download used by config URL imports and plugin
//! installs.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};

/// Downloads `url` into `dest` with a hard timeout and an optional size cap.
///
/// The cap makes curl abort the transfer instead of writing an unbounded
/// file. It is defense in depth: curl can only enforce it when it can track
/// the transfer size, so callers with an exact limit must still check the
/// final file size.
pub fn download_to_file(
    url: &str,
    dest: &Path,
    timeout: Duration,
    max_bytes: Option<u64>,
) -> Result<()> {
    let mut command = Command::new("curl");
    crate::platform::environment::configure_command(&mut command);
    command.args(["-fsSL", "--max-time", &timeout.as_secs().to_string()]);
    if let Some(limit) = max_bytes {
        command.args(["--max-filesize", &limit.to_string()]);
    }
    let status = command
        .arg("-o")
        .arg(dest)
        .arg(url)
        .status()
        .with_context(|| format!("download {url}"))?;
    if !status.success() {
        bail!("download {url} failed with status {status}");
    }
    Ok(())
}
