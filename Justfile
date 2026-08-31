# Clipboard Transformer development and packaging recipes.
# Manual: https://just.systems/man/en/
#
# Recipe names are namespaced by action so `just --list` reads as an explicit
# menu: `check-*` verifies without mutating, `test-*` runs test binaries,
# `build-*` produces binaries or bundles, `package-*` produces distributable
# artifacts, `install-*` replaces an installed copy, `gen-*` refreshes
# committed generated files, `run-*` runs the CLI, and `probe-*` runs the
# ignored performance probes. `just ci` is the local mirror of the GitHub CI
# workflow.
#
# Submodules are intentionally not used: a `just` module inherits neither the
# parent's variables nor its `set shell`, so every module file would have to
# duplicate the shared paths and the platform shell settings below.

set minimum-version := "1.56.0"
set ignore-comments

# Local, git-ignored `.env` in the repository root, so packaging credentials
# like APPLE_SIGNING_IDENTITY and APPLE_KEYCHAIN_PROFILE do not have to be
# exported by hand. just loads exactly one dotenv file, and a variable already
# present in the real environment wins over the file, so an inline
# `APPLE_SIGNING_IDENTITY=... just package-macos` still overrides it. This is
# unrelated to the `.env` the application itself reads next to its config.
set dotenv-load

[unix]
set shell := ["sh", "-cu"]

[windows]
set shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

schema_file := "target/generated/clipboard-transformer.schema.json"
app_dir := "target/macos/Clipboard Transformer.app"
installed_app_dir := "/Applications/Clipboard Transformer.app"
packager_app_dir := "target/packager/Clipboard Transformer.app"
packager_resources_dir := "target/packager-resources"
windows_standalone_dir := "target/windows/standalone"
app_icon_source := "assets/AppIcon.icon"
macos_icon_dir := "assets/generated/macos"
windows_icon_dir := "assets/generated/windows"
linux_icon_dir := "assets/generated/linux"
linux_packager_config := "package/linux/Packager.toml"
flatpak_manifest := "package/flatpak/dev.jag_k.clipboard_transformer.json"
flatpak_app_id := "dev.jag_k.clipboard_transformer"
macos_packager_config := if env("APPLE_SIGNING_IDENTITY", "") == "" { "Packager.toml" } else { "Packager.local.toml" }
bin := "clipboard-transformer"
app_bin := "clipboard-transformer-app"
release_app_bin := "target/release/" + app_bin
windows_release_bin := "target/release/" + bin + ".exe"
windows_app_bin := "target/release/" + app_bin + ".exe"
windows_standalone_target := "x86_64-pc-windows-msvc"
windows_standalone_bin := "target/" + windows_standalone_target + "/release/" + bin + ".exe"

# List recipes when invoked as `just` with no arguments.
[private]
default:
    @just --list --unsorted

# Local mirror of CI ---------------------------------------------------------------

# Every step here has a matching step in .github/workflows/ci.yml. Plugin
# runtime tests are excluded because they need the wasm32-wasip1 target; CI
# covers them in a separate job, locally use `just test-plugins`.
[doc('Run every portable check GitHub CI runs')]
[group('ci')]
ci: check-fmt check check-cli check-wasm check-clippy check-clippy-cli test
    @echo ok

# Verification --------------------------------------------------------------------

[doc('Check Rust formatting without writing')]
[group('check')]
check-fmt:
    cargo fmt --all -- --check

[doc('Type-check the workspace')]
[group('check')]
check:
    cargo check --locked --all-targets

# The default-feature build already covers the desktop app; this is the
# configuration --all-targets does not see: the plain CLI without `desktop`.
[doc('Type-check the CLI without default features')]
[group('check')]
check-cli:
    cargo check --locked --no-default-features --lib --bin {{ bin }}

[doc('Type-check portable crates for browser WASM')]
[group('check')]
check-wasm:
    cargo check --locked -p ct-clipboard --target wasm32-unknown-unknown
    cargo check --locked -p ct-core --target wasm32-unknown-unknown
    cargo check --locked -p ct-config --target wasm32-unknown-unknown
    cargo check --locked -p ct-plugin-api --target wasm32-unknown-unknown

[doc('Lint with clippy (warnings denied)')]
[group('check')]
check-clippy:
    cargo clippy --locked --all-targets -- -D warnings

# `check-clippy` already covers the desktop app (desktop is a default feature);
# this lints the configuration --all-targets does not see: the plain CLI.
[doc('Lint the CLI binary without default features')]
[group('check')]
check-clippy-cli:
    cargo clippy --locked --no-default-features --bin {{ bin }} -- -D warnings

# Needs `cargo install cargo-deny --locked`. Not part of `just ci`: it fetches
# the advisory database over the network. Configuration is in deny.toml.
[doc('Check dependency advisories, licenses, sources, and duplicates')]
[group('check')]
check-deny:
    cargo deny check

# The MSRV is read from Cargo.toml instead of repeated here, so `rust-version`
# stays the single declaration. Not part of `just ci`: it installs a second
# toolchain, which the ordinary edit loop should not do behind your back.
[doc('Type-check the workspace on the MSRV declared in Cargo.toml')]
[group('check')]
[unix]
check-msrv:
    #!/usr/bin/env bash
    set -euo pipefail
    msrv="$(sed -nE 's/^rust-version = "([^"]+)"$/\1/p' Cargo.toml | head -n 1)"
    if [[ -z "$msrv" ]]; then
      echo "Cargo.toml declares no rust-version" >&2
      exit 1
    fi
    if ! rustup run "$msrv" rustc --version >/dev/null 2>&1; then
      rustup toolchain install "$msrv" --profile minimal
    fi
    echo "checking against declared MSRV $msrv"
    cargo "+$msrv" check --locked --all-targets

[doc('Type-check Windows and Linux platform code from a macOS host')]
[group('check')]
[macos]
check-cross:
    #!/usr/bin/env bash
    set -euo pipefail
    # `cargo check` never links, so cross-checking only needs a C compiler for
    # the C dependencies wasmtime pulls in. Requires `brew install mingw-w64 zig`
    # plus both rustup targets. mingw-w64 is detected automatically for
    # windows-gnu; zig needs a shim because it spells targets arch-os-abi and
    # rejects cc-rs's `--target=x86_64-unknown-linux-gnu`.
    #
    # windows-gnu is not the release target (that is windows-msvc, which needs
    # the MSVC headers), so cfg(target_env) branches stay CI-only. Everything
    # else type-checks, which is what makes platform edits non-blind.
    wrapper="$PWD/target/cross/zig-cc-linux"
    mkdir -p "$(dirname "$wrapper")"
    printf '%s\n' \
      '#!/usr/bin/env bash' \
      'args=()' \
      'for a in "$@"; do [[ $a == --target=* ]] && continue; args+=("$a"); done' \
      'exec zig cc -target x86_64-linux-gnu "${args[@]}"' > "$wrapper"
    chmod +x "$wrapper"
    CC_x86_64_unknown_linux_gnu="$wrapper" \
      cargo check --locked --all-targets --target x86_64-unknown-linux-gnu
    cargo check --locked --all-targets --target x86_64-pc-windows-gnu

# Tests ---------------------------------------------------------------------------

[doc('Run the test suite')]
[group('test')]
test:
    cargo test --locked

[doc('Run the test suite including the example plugin runtime tests')]
[group('test')]
test-plugins: build-example-plugin
    cargo test --locked

# These two need a real session, so they are separate from `just test`: CI runs
# them under Xvfb and under a headless wlroots compositor.
[doc('Exercise the real X11 clipboard backend (needs DISPLAY)')]
[group('test')]
[linux]
test-linux-x11:
    cargo test --locked --test linux_x11

[doc('Exercise the real wlr-data-control clipboard backend (needs WAYLAND_DISPLAY)')]
[group('test')]
[linux]
test-linux-wayland:
    cargo test --locked --test linux_wayland

# Formatting and code generation ---------------------------------------------------

[doc('Format Rust sources')]
[group('gen')]
fmt:
    cargo fmt --all

[doc('Regenerate all checked-in plugin authoring schemas')]
[group('gen')]
gen-schemas:
    cargo run --locked -p codegen -- schemas

[doc('Re-rasterize the committed tray icons from assets/tray.svg')]
[group('gen')]
gen-icons *args:
    cargo run --locked -p codegen -- icons {{ args }}

[doc('Write the config JSON Schema under target/generated/')]
[group('gen')]
gen-config-schema:
    cargo run --locked --bin {{ bin }} -- config schema --output {{ quote(schema_file) }}

[doc('Generate the ignored Flatpak Cargo source list from Cargo.lock')]
[group('gen')]
[unix]
gen-flatpak-sources:
    package/flatpak/update-cargo-sources.sh

[arg('force', long='force', value='true')]
[doc('Compile AppIcon.icon into macOS/Linux/Windows icon assets')]
[group('gen')]
[macos]
gen-app-icon force="false":
    #!/usr/bin/env bash
    set -euo pipefail
    macos_icon_dir={{ quote(macos_icon_dir) }}
    windows_icon_dir={{ quote(windows_icon_dir) }}
    linux_icon_dir={{ quote(linux_icon_dir) }}
    app_icon_source={{ quote(app_icon_source) }}

    if [[ {{ quote(force) }} != "true" \
      && -f "$macos_icon_dir/Assets.car" \
      && -f "$macos_icon_dir/AppIcon.icns" \
      && -f "$linux_icon_dir/app-icon.png" \
      && -f "$windows_icon_dir/app-icon.ico" ]]; then
      echo "app icon already exists; use 'just gen-app-icon --force' to regenerate"
      exit 0
    fi

    mkdir -p "$macos_icon_dir" "$windows_icon_dir" "$linux_icon_dir"
    xcrun actool \
      --compile "$macos_icon_dir" \
      --platform macosx \
      --minimum-deployment-target 13.0 \
      --app-icon AppIcon \
      --output-partial-info-plist "$macos_icon_dir/Icon.partial.plist" \
      "$app_icon_source"
    iconset="$(mktemp -d "${TMPDIR:-/tmp}/clipboard-transformer-icon.XXXXXX")/AppIcon.iconset"
    iconutil --convert iconset --output "$iconset" "$macos_icon_dir/AppIcon.icns"
    cp "$iconset/icon_128x128@2x.png" "$linux_icon_dir/app-icon.png"
    sips --setProperty format ico \
      "$linux_icon_dir/app-icon.png" \
      --out "$windows_icon_dir/app-icon.ico" >/dev/null

# Portable builds -----------------------------------------------------------------

[doc('Build the release app and CLI binaries')]
[group('build')]
build-release:
    cargo build --release --locked --no-default-features --bin {{ bin }}
    cargo build --release --locked --features desktop --bin {{ app_bin }}

[doc('Build the XTP-generated example GitLab plugin')]
[group('build')]
build-example-plugin:
    cargo build --manifest-path plugins/gitlab-link/Cargo.toml --target wasm32-wasip1 --release

# Local CLI helpers ---------------------------------------------------------------

[doc('Run `clipboard-transformer doctor`')]
[group('run')]
run-doctor:
    cargo run --locked --bin {{ bin }} -- doctor

[doc('Print resolved config/state/cache paths')]
[group('run')]
run-paths:
    cargo run --locked --bin {{ bin }} -- paths

[doc('Create the default YAML config and local schema')]
[group('run')]
run-init:
    cargo run --locked --bin {{ bin }} -- init

[arg('config', help='Path to a YAML or TOML config')]
[doc('Validate a config file')]
[group('run')]
run-validate config="fixtures/config.yaml":
    cargo run --locked --bin {{ bin }} -- validate --config-file {{ quote(config) }}

# Performance probes --------------------------------------------------------------

[arg('iterations', help='Number of overlapping plugin runtime replacements')]
[doc('Report current RSS while repeatedly replacing the example plugin runtime')]
[group('probe')]
[unix]
probe-plugin-reload-memory iterations="20": build-example-plugin
    PLUGIN_RELOAD_PROBE_ITERATIONS={{ quote(iterations) }} cargo test --locked --test plugins_runtime repeated_plugin_replacement_reports_current_rss -- --ignored --nocapture

[arg('cycles', help='Number of whole rule-tree replacements')]
[arg('rules', help='Number of URL cleanup rules per generated tree')]
[arg('corpus', help='Number of clipboard values per full pass')]
[doc('Soak generated rule trees and report RSS around repeated hot reloads')]
[group('probe')]
[unix]
probe-rule-reload-memory cycles="4" rules="1024" corpus="1000":
    RULE_MEMORY_PROBE_CYCLES={{ quote(cycles) }} RULE_MEMORY_PROBE_RULES={{ quote(rules) }} RULE_MEMORY_PROBE_CORPUS={{ quote(corpus) }} cargo test --release --locked --test rules_memory -- --ignored --nocapture

# macOS application and packaging -------------------------------------------------

[group('macos')]
[macos]
[private]
codesign path:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -n "${APPLE_CODESIGN_IDENTITY:-}" ]]; then
      codesign --force --options runtime --timestamp --sign "$APPLE_CODESIGN_IDENTITY" {{ quote(path) }}
    else
      codesign --force --sign - {{ quote(path) }}
    fi

[group('macos')]
[macos]
[private]
require-packager:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-packager >/dev/null 2>&1; then
      echo "cargo-packager is not installed." >&2
      echo "Install it with: cargo install cargo-packager --locked" >&2
      exit 1
    fi

# cargo-packager's pre-package hook is intentionally not used because hooks are
# global rather than platform-scoped. The release workflow calls this recipe
# directly and then signs the staged CLI, so the two paths cannot drift.
[doc('Prepare the native macOS binary and package resources')]
[group('macos')]
[macos]
prepare-package-macos: build-release gen-app-icon
    cp {{ quote(release_app_bin) }} {{ quote("target/release/Clipboard Transformer") }}
    rm -rf {{ quote(packager_resources_dir) }}
    mkdir -p {{ quote(packager_resources_dir) }}
    cp {{ quote(macos_icon_dir + "/Assets.car") }} {{ quote(packager_resources_dir + "/Assets.car") }}
    cp {{ quote(macos_icon_dir + "/AppIcon.icns") }} {{ quote(packager_resources_dir + "/AppIcon.icns") }}

[doc('Build a local .app under target/macos/')]
[group('macos')]
[macos]
build-app: build-release gen-app-icon
    rm -rf {{ quote(app_dir) }}
    mkdir -p {{ quote(app_dir + "/Contents/MacOS") }} {{ quote(app_dir + "/Contents/Resources") }}
    cp package/macos/Info.plist {{ quote(app_dir + "/Contents/Info.plist") }}
    cp {{ quote(release_app_bin) }} {{ quote(app_dir + "/Contents/MacOS/Clipboard Transformer") }}
    cp {{ quote(macos_icon_dir + "/Assets.car") }} {{ quote(app_dir + "/Contents/Resources/Assets.car") }}
    cp {{ quote(macos_icon_dir + "/AppIcon.icns") }} {{ quote(app_dir + "/Contents/Resources/AppIcon.icns") }}
    just codesign {{ quote(app_dir) }}
    @echo {{ quote(app_dir) }}

[confirm('Replace /Applications/Clipboard Transformer.app?')]
[doc('Install the local .app into /Applications')]
[group('macos')]
[macos]
install-app: build-app
    #!/usr/bin/env bash
    set -euo pipefail
    app_dir={{ quote(app_dir) }}
    packager_app_dir={{ quote(packager_app_dir) }}
    installed_app_dir={{ quote(installed_app_dir) }}
    lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

    pkill -f clipboard-transformer || true
    pkill -f "Clipboard Transformer.app/Contents/MacOS/Clipboard Transformer" || true
    "$lsregister" -u "$(pwd)/$app_dir" 2>/dev/null || true
    "$lsregister" -u "$(pwd)/$packager_app_dir" 2>/dev/null || true
    rm -rf "$installed_app_dir"
    ditto "$app_dir" "$installed_app_dir"
    "$lsregister" -f "$installed_app_dir"
    echo "$installed_app_dir"

# cargo-packager 0.11.8 reads the Developer ID identity only from its config
# file: there is no environment variable and no CLI flag, and a raw JSON
# `--config` replaces the whole configuration instead of overlaying it. So an
# identity passed through APPLE_SIGNING_IDENTITY is written into a git-ignored
# copy and the tracked Packager.toml keeps its commented placeholder. The copy
# has to stay in the repository root: cargo-packager chdirs to the config
# file's parent directory, so every relative path in it would break elsewhere.
[macos]
[private]
_packager-config:
    #!/usr/bin/env bash
    set -euo pipefail
    identity="${APPLE_SIGNING_IDENTITY:-}"
    if [[ -z "${identity}" ]]; then
      rm -f Packager.local.toml
      exit 0
    fi
    escaped="$(printf '%s' "${identity}" | sed -e 's/[&|\\]/\\&/g')"
    sed -E "s|^#? ?signingIdentity = .*|signingIdentity = \"${escaped}\"|" \
      Packager.toml > Packager.local.toml
    grep -q '^signingIdentity = ' Packager.local.toml

# cargo-packager signs with the config's signingIdentity itself; re-running
# `just codesign` here would clobber that Developer ID signature with an ad-hoc
# one and drop the entitlements.
[macos]
[private]
_package formats: prepare-package-macos require-packager _packager-config
    cargo-packager --config {{ quote(macos_packager_config) }} {{ formats }}

[doc('Build a release .app with cargo-packager')]
[group('macos')]
[macos]
package-app: (_package "--formats app")
    @echo {{ quote(packager_app_dir) }}

[doc('Build release .app and DMG artifacts with cargo-packager')]
[group('macos')]
[macos]
package-macos: (_package "--formats app --formats dmg")
    @echo "target/packager"

# Linux application and packaging -------------------------------------------------

[group('linux')]
[linux]
[private]
require-packager-linux:
    #!/usr/bin/env sh
    set -eu
    if ! command -v cargo-packager >/dev/null 2>&1; then
      echo "cargo-packager is not installed." >&2
      echo "Install it with: cargo install cargo-packager --locked --version 0.11.8" >&2
      exit 1
    fi

[group('linux')]
[linux]
[private]
require-generate-rpm:
    #!/usr/bin/env sh
    set -eu
    if ! cargo generate-rpm --version >/dev/null 2>&1; then
      echo "cargo-generate-rpm is not installed." >&2
      echo "Install it with: cargo install cargo-generate-rpm --locked --version 0.21.0" >&2
      exit 1
    fi

# The release workflow calls this recipe directly so the packaged build flags
# cannot drift from the local ones.
[doc('Build and stage the native Linux desktop app and CLI')]
[group('linux')]
[linux]
prepare-package-linux:
    cargo build --release --locked --no-default-features --bin {{ bin }}
    cargo build --release --locked --features desktop --bin {{ app_bin }}

[linux]
[private]
_package-linux +formats: prepare-package-linux require-packager-linux
    APPIMAGE_EXTRACT_AND_RUN=1 cargo-packager --config {{ quote(linux_packager_config) }} {{ formats }}

[doc('Build the Linux Debian package')]
[group('linux')]
[linux]
package-linux-deb: (_package-linux "--formats deb")
    @echo "target/packager"

[doc('Build the Linux AppImage')]
[group('linux')]
[linux]
package-linux-appimage: (_package-linux "--formats appimage")
    @echo "target/packager"

[doc('Build the Arch Linux Pacman package and PKGBUILD')]
[group('linux')]
[linux]
package-linux-pacman: (_package-linux "--formats pacman")
    package/linux/add-pacman-optdepends.sh target/packager/PKGBUILD
    @echo "target/packager"

[doc('Build the Linux RPM package')]
[group('linux')]
[linux]
package-linux-rpm: prepare-package-linux require-generate-rpm
    cargo generate-rpm -p apps/desktop
    @echo "target/generate-rpm"

# One cargo-packager invocation for all three of its formats, matching the
# release workflow. Running it once per format would repeat the staging work and
# leave the local path different from the packaged one.
[doc('Build AppImage, DEB, Pacman/PKGBUILD, and RPM Linux artifacts')]
[group('linux')]
[linux]
package-linux: (_package-linux "--formats deb --formats appimage --formats pacman") require-generate-rpm
    package/linux/add-pacman-optdepends.sh target/packager/PKGBUILD
    cargo generate-rpm -p apps/desktop
    @echo "target/packager"
    @echo "target/generate-rpm"

[group('linux')]
[linux]
[private]
require-flatpak-builder:
    #!/usr/bin/env sh
    set -eu
    if ! command -v flatpak >/dev/null 2>&1 || ! command -v flatpak-builder >/dev/null 2>&1; then
      echo "flatpak and flatpak-builder are required." >&2
      exit 1
    fi

[doc('Build a local Flatpak bundle (requires the Flathub runtime remote)')]
[group('linux')]
[linux]
package-flatpak: require-flatpak-builder gen-flatpak-sources
    #!/usr/bin/env bash
    set -euo pipefail
    version="$(sed -nE 's/^version = "([^"]+)"$/\1/p' Cargo.toml | head -n 1)"
    build_dir="target/flatpak-build"
    repo_dir="target/flatpak-repo"
    bundle="target/clipboard-transformer-${version}-$(uname -m).flatpak"
    rm -rf "$build_dir" "$repo_dir"
    flatpak-builder --user --force-clean --install-deps-from=flathub \
      --default-branch=stable --repo="$repo_dir" \
      "$build_dir" {{ quote(flatpak_manifest) }}
    flatpak build-bundle "$repo_dir" "$bundle" {{ quote(flatpak_app_id) }} stable
    sha256sum "$bundle" > "$bundle.sha256"
    echo "$bundle"

# Windows artifacts (run on Windows) ---------------------------------------------

[group('windows')]
[private]
[windows]
require-packager:
    if (-not (Get-Command cargo-packager -ErrorAction SilentlyContinue)) { \
      throw "cargo-packager is not installed; run: cargo install cargo-packager --locked" \
    }

# The resource staging directory is shared by the single cross-platform
# Packager.toml and contains only files intended for the current platform. The
# release workflow calls this recipe directly so the two paths cannot drift.
[doc('Prepare the native Windows binary and package resources')]
[group('windows')]
[windows]
prepare-package-windows:
    cargo build --release --locked --no-default-features --bin {{ bin }}
    cargo build --release --locked --features desktop --bin {{ app_bin }}
    Copy-Item {{ windows_app_bin }} "target/release/Clipboard Transformer.exe"
    if (Test-Path "{{ packager_resources_dir }}") { Remove-Item -Recurse -Force "{{ packager_resources_dir }}" }
    New-Item -ItemType Directory -Force "{{ packager_resources_dir }}/bin" | Out-Null
    Copy-Item {{ windows_release_bin }} "{{ packager_resources_dir }}/bin/clipboard-transformer.exe"

[doc('Build and stage the standalone Windows x86_64 executable')]
[group('windows')]
[windows]
build-windows-standalone:
    cargo build --release --locked --no-default-features --bin {{ bin }} --target {{ windows_standalone_target }}
    if (Test-Path "{{ windows_standalone_dir }}") { Remove-Item -Recurse -Force "{{ windows_standalone_dir }}" }
    New-Item -ItemType Directory -Force "{{ windows_standalone_dir }}" | Out-Null
    Copy-Item {{ windows_standalone_bin }} "{{ windows_standalone_dir }}/clipboard-transformer.exe"
    Copy-Item {{ windows_icon_dir }}/app-icon.ico "{{ windows_standalone_dir }}/app-icon.ico"

[doc('Build the Windows x86_64 MSI through cargo-packager and WiX')]
[group('windows')]
[windows]
package-windows-msi: prepare-package-windows require-packager
    cargo-packager --config Packager.toml --formats wix
    Write-Output "target/packager"

[confirm('Build and install the Clipboard Transformer MSI?')]
[doc('Build and install the Windows MSI into Program Files')]
[group('windows')]
[windows]
install-app-windows: package-windows-msi
    $msi = Get-ChildItem "target/packager/*.msi" | Sort-Object LastWriteTime -Descending | Select-Object -First 1; \
    if (-not $msi) { throw "cargo-packager did not produce an MSI" }; \
    Get-Process -Name "Clipboard Transformer", "clipboard-transformer", "clipboard-transformer-app" -ErrorAction SilentlyContinue | Stop-Process -Force; \
    $log = "target/msi-install.log"; \
    $arguments = @("/i", ('"{0}"' -f $msi.FullName), "/qn", "/norestart", "/l*v", ('"{0}"' -f $log)); \
    $installer = Start-Process "msiexec.exe" -Verb RunAs -ArgumentList $arguments -Wait -PassThru; \
    if ($installer.ExitCode -notin @(0, 3010)) { throw "msiexec failed with exit code $($installer.ExitCode); see $log" }; \
    Write-Output "C:\Program Files\Clipboard Transformer"
