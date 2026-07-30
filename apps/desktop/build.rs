use std::env;
use std::path::{Path, PathBuf};

const WINDOWS_APP_ICON: &str = "assets/generated/windows/app-icon.ico";

/// Embeds the Windows application icon into `clipboard-transformer-app.exe`.
///
/// This must stay in the package that produces the executable. `winresource`
/// emits `cargo:rustc-link-arg` on MSVC targets, and Cargo applies that only to
/// the targets of the package whose build script printed it — a build script in
/// a library dependency would compile fine and silently embed nothing.
fn main() {
    let icon = workspace_root().join(WINDOWS_APP_ICON);
    println!("cargo:rerun-if-changed={}", icon.display());

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // The portable/MSI pipeline renames the Cargo-produced executable after
    // linking. Set the PE version-resource strings explicitly so Windows UI
    // (including Task Manager) uses the product name instead of this package's
    // internal Cargo name, `ct-desktop`.
    winresource::WindowsResource::new()
        .set("ProductName", "Clipboard Transformer")
        .set("FileDescription", "Clipboard Transformer")
        .set("InternalName", "Clipboard Transformer")
        .set("OriginalFilename", "Clipboard Transformer.exe")
        .set_icon(icon.to_str().expect("Windows icon path is UTF-8"))
        .compile()
        .expect("embed Windows application icon");
}

fn workspace_root() -> PathBuf {
    if let Some(root) = env::var_os("CT_WORKSPACE_ROOT") {
        return PathBuf::from(root);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve workspace root")
}
