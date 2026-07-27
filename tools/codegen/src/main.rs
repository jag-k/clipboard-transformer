//! Generators for artifacts that are committed to the repository.
//!
//! These are explicit development tools, never build scripts: the artifacts are
//! checked in and consumed outside Cargo (plugin authors' editors, the XTP
//! guest template, `include!` sites), and Cargo forbids build scripts from
//! writing outside `OUT_DIR`. Staleness is caught by the drift tests in each
//! module rather than by regenerating on every build.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

mod icons;
mod schemas;
mod xtp_schema;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    let force = args.any(|arg| arg == "--force" || arg == "-f");

    match command.as_deref() {
        Some("schemas") => schemas::generate(),
        Some("icons") => icons::generate(force),
        Some("all") | None => {
            schemas::generate()?;
            icons::generate(force)
        }
        Some(other) => bail!("unknown command {other:?}; expected schemas, icons, or all"),
    }
}

/// Workspace root from `.cargo/config.toml`, falling back to walking up from
/// this package. Never hardcode `../..` at a call site.
pub fn workspace_root() -> PathBuf {
    if let Some(root) = std::env::var_os("CT_WORKSPACE_ROOT") {
        return PathBuf::from(root);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve workspace root")
}

/// Writes only when the contents differ, so regeneration leaves a clean working
/// tree when nothing changed.
pub fn write_if_changed(path: &Path, contents: &[u8]) -> Result<()> {
    if std::fs::read(path).ok().as_deref() == Some(contents) {
        println!("unchanged {}", path.display());
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    println!("wrote {}", path.display());
    Ok(())
}
