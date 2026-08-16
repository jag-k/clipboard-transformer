# Verifying packages and installers

How to verify every packaging/publishing surface — locally on macOS/Linux, on
a Windows machine, and through CI — **without publishing a release**. Written
for both humans and agents; every command is copy-pasteable from the repo
root. `releasing.md` describes what the release pipeline produces;
`ci-configuration.md` inventories all CI secrets and variables;
`macos-signing.md` covers signing/notarization credentials in depth.

## What exists and where it is tested

| Surface | Built by | Verified on |
| --- | --- | --- |
| macOS `.app` + DMG | `just package-app` / `just package-macos` (cargo-packager) | macOS, locally |
| macOS signing/notarization | `build-macos-packages.yml`; same steps run locally | macOS, locally |
| Homebrew cask | `package/homebrew/clipboard-transformer.rb` + `publish-homebrew.yml` | macOS and Linux, locally |
| Windows MSI | `build-windows-msi.yml` / `just`-equivalents (cargo-packager + WiX) | Windows VM or CI artifact |
| Windows portable ZIP | `build-windows-msi.yml` staging step | Windows VM |
| Scoop manifest | `package/scoop/clipboard-transformer.json` + `publish-scoop.yml` | Windows VM |
| WinGet manifest | `publish-winget.yml` (wingetcreate against the MSI) | Windows VM |
| Linux AppImage, DEB, Pacman/PKGBUILD | `just package-linux` / `build-linux-packages.yml` | matching Linux VM |
| Linux RPM | `just package-linux-rpm` / `build-linux-packages.yml` | Fedora/openSUSE VM |
| Linux Flatpak bundle | `just package-flatpak` / `build-flatpak.yml` | Flatpak-enabled Linux desktop |
| Nix Linux/macOS package | `nix build` / `nix.yml` | Linux and macOS CI |
| Release orchestration | `release.yml` (tag-driven) | CI only |

## macOS: fully local

Build and inspect the bundles:

```sh
just package-app     # .app only; prints the bundle path
just package-macos   # .app + DMG under target/packager
```

Signing is off by default (`signingIdentity` is commented out in
`Packager.toml`). To test a signed build, export `APPLE_SIGNING_IDENTITY` with
an identity present in the login keychain — the recipes then package from a
git-ignored `Packager.local.toml` carrying it — then verify:

```sh
codesign --verify --deep --strict --verbose=2 "<path>.app"
xcrun stapler validate "<path>.app"            # after notarization only
spctl --assess --type execute --verbose=2 "<path>.app"
```

`spctl` passes only after notarization + stapling. The full
sign-notarize-staple ritual, credentials, and the Wasmtime entitlement caveat
are in `macos-signing.md`. Note: the very first notarization submission
from a new Developer ID team can take up to ~24 hours; later submissions
normally finish in minutes. Check pending submissions with
`xcrun notarytool history --keychain-profile "<profile>"`.

### Homebrew cask locally

On macOS, the cask installs from the release `-homebrew.zip` (the `.app` plus
the standalone CLI). On Linux x86_64, the same cask installs a bundle
containing the AppImage and standalone CLI. To test the macOS path without a
release:

```sh
# 1. Build the zip the way CI does (app + CLI + LICENSE in one folder):
just package-app
mkdir -p target/homebrew-root
ditto "target/packager/Clipboard Transformer.app" "target/homebrew-root/Clipboard Transformer.app"
cp target/release/clipboard-transformer LICENSE target/homebrew-root/
ditto -c -k --sequesterRsrc target/homebrew-root target/homebrew-local.zip

# 2. Point a copy of the cask at it:
cp package/homebrew/clipboard-transformer.rb /tmp/clipboard-transformer.rb
shasum -a 256 target/homebrew-local.zip   # replace both macOS sha256 values
# replace the url line with: url "file://#{Dir.pwd}/target/homebrew-local.zip"

# 3. Lint and install:
brew style /tmp/clipboard-transformer.rb
brew audit --cask /tmp/clipboard-transformer.rb || true   # audit needs a tap context; style is the hard gate
brew install --cask /tmp/clipboard-transformer.rb
```

The `ruby` rendering snippet from `publish-homebrew.yml` can be run directly
in a shell with fake macOS and Linux sha256 values to preview the published
cask. On Linux, point the `on_linux` URL at a locally built
`clipboard-transformer-<version>-x86_64-linux-homebrew.tar.xz`, replace its
hash, then run the same style, audit, and install commands. The archive must
contain `Clipboard Transformer.AppImage`, `clipboard-transformer`, and
`LICENSE`.

## Windows: VM or real machine

Use any Windows 11 machine or the free Microsoft dev VM (runs under
UTM/Parallels/VMware on a Mac). Artifacts come from a CI dry run (below) or
are built on the VM with the same commands as
`.github/workflows/build-windows-msi.yml`.

**MSI:**

```powershell
$msi = (Resolve-Path .\clipboard-transformer-<version>-x86_64.msi).Path
$previousMsi = (Resolve-Path .\clipboard-transformer-0.1.0-x86_64.msi).Path
.\package\windows\verify-msi.ps1 `
  -MsiPath $msi `
  -PreviousMsiPath $previousMsi
```

Omit `-PreviousMsiPath` for a clean-install-only check. The script requests
elevation when run locally, performs non-interactive install and uninstall, and
writes verbose logs under `target/msi-verification`. GitHub-hosted Windows
runners already run as administrators with UAC disabled. A non-elevated
GitHub Actions self-hosted runner fails immediately instead of waiting for a
UAC dialog that nobody can accept.

With `-PreviousMsiPath`, the script installs that MSI before the candidate and
therefore verifies the real upgrade path. It then checks:

- `Clipboard Transformer.exe` and `bin\clipboard-transformer.exe`;
- `clipboard-transformer --version` through the installed CLI;
- the system `PATH` entry for the CLI `bin` directory;
- the all-users Start Menu shortcut;
- the HKLM toast activator registration;
- removal of all installer-owned files, registrations, shortcut, and `PATH`
  entry.

The `CliPath` component is owned directly by cargo-packager's optional
`Environment` feature. Do not replace that ownership with a component
condition based on `&Environment`: component conditions are evaluated before
`MigrateFeatureStates` during a major upgrade and can silently skip the `PATH`
entry even when the feature is migrated as selected.

Also run the tray app once and exercise an actionable notification manually;
the CI smoke test does not start the GUI. Before publishing a changed MSI,
install the previous public version first and upgrade to the candidate so
feature migration and `RemoveExistingProducts` are covered on a real machine.

For direct diagnosis, use an elevated silent install and keep the verbose log:

```powershell
$installLog = Join-Path $PWD "install.log"
$process = Start-Process msiexec.exe `
  -Verb RunAs `
  -ArgumentList "/i `"$msi`" /qn /norestart /l*v `"$installLog`"" `
  -Wait -PassThru
$process.ExitCode
```

Exit `0` is success and `3010` is success with a requested reboot. A bare
`1603` is only a summary; inspect the lines preceding `Return value 3`.
Per-machine silent installation from a non-elevated shell commonly returns
`1603`, so preserve `-Verb RunAs` in manual checks.

**Scoop from a local manifest** (exercises `post_install`/`post_uninstall`
hooks — they must work under Windows PowerShell 5.1, which Scoop uses):

```powershell
# Serve the portable zip so the manifest URL is fetchable:
python -m http.server 8000   # from the folder containing the zip
# In a copy of package/scoop/clipboard-transformer.json set:
#   architecture."64bit".url  = "http://localhost:8000/<zip name>"
#   architecture."64bit".hash = <sha256 of the zip>
scoop install .\clipboard-transformer.json
scoop uninstall clipboard-transformer   # then confirm Run keys and the app-owned Start Menu shortcut are gone
```

**WinGet manifest validation:**

```powershell
winget install Microsoft.WingetCreate
winget settings --enable LocalManifestFiles

# First accepted version:
wingetcreate new "https://github.com/jag-k/clipboard-transformer/releases/download/v<version>/clipboard-transformer-<version>-x86_64.msi"

# Later versions:
wingetcreate update JagK.ClipboardTransformer --urls <msi URL> --version <version>

winget validate --manifest <generated manifest dir>
winget install --manifest <generated manifest dir> --silent
winget uninstall --id JagK.ClipboardTransformer --silent
```

The first manifest uses `JagK.ClipboardTransformer`, `en-US`, publisher
`jag-k`, package name `Clipboard Transformer`, license `MPL-2.0`, installer
type `wix`, and scope `machine`. WinGetCreate extracts the architecture,
product code, version, and SHA-256 from the MSI. Test the generated manifest in
Windows Sandbox before submitting it.

## Linux: VM or real desktop

Build the complete local matrix on Linux:

```sh
cargo install cargo-packager --locked --version 0.11.8
cargo install cargo-generate-rpm --locked --version 0.21.0
just package-linux
```

Inspect package contents before installation:

```sh
dpkg-deb --contents target/packager/*.deb
tar -tf target/packager/*.tar.gz
rpm -qpl target/generate-rpm/*.rpm
```

Each installed native package must contain both executables, the
`dev.jag-k.clipboard-transformer.desktop` entry, its icon, and the matching
D-Bus service. Verify `doctor`, tray actions, notification actions, autostart,
and uninstall on a real desktop. Clipboard coverage must include X11,
GNOME/XWayland bridging, and a compositor exposing native data-control. A
Wayland session with neither data-control nor XWayland must show the fatal
notification and leave no watcher process running.

Build the Flatpak bundle after adding Flathub as a runtime remote:

```sh
flatpak remote-add --user --if-not-exists flathub \
  https://flathub.org/repo/flathub.flatpakrepo
just package-flatpak
flatpak install --user ./target/clipboard-transformer-*.flatpak
flatpak run dev.jag_k.clipboard_transformer
```

Verify X11, native data-control Wayland, the tray, notification actions, URL
imports, and sandbox-local config. Confirm that the autostart action is absent
and that a `shell` rule cannot silently invoke an arbitrary host executable.

After bootstrapping `jag-k/flatpak-repo`, test repository publication with a
stable release asset and the manual workflow:

```sh
version=0.1.3 # replace with a release that contains the Flatpak bundle
gh workflow run publish-flatpak.yml -f version="$version" -f tag="v$version"
flatpak remote-add --user --if-not-exists jag-k \
  https://jag-k.github.io/flatpak-repo/jag-k.flatpakrepo
flatpak remote-ls jag-k
flatpak install --user jag-k dev.jag_k.clipboard_transformer
```

Verify that the remote summary and application ref are GPG-verified. Publish a
new test version and confirm `flatpak update` advances an existing installation
without re-adding the remote.

Build the Nix package on both Linux and macOS:

```sh
nix build --print-build-logs .#default
result/bin/clipboard-transformer --version
```

On macOS also launch `result/Applications/Clipboard Transformer.app`; on Linux
run `result/bin/clipboard-transformer doctor` in the graphical session.

## CI without publishing

All four build workflows accept `workflow_dispatch` in addition to
`workflow_call`, so each one runs on its own without the release job and without
a temporary wrapper workflow:

```sh
gh workflow run build-macos-packages.yml -f version=0.1.0
gh workflow run build-windows-msi.yml -f version=0.1.0
gh workflow run build-linux-packages.yml -f version=0.1.0
gh workflow run build-flatpak.yml -f version=0.1.0
```

Run them from the Actions tab or as above; download signed/notarized/attested
artifacts from the run page. Nothing is released and no manifest repos are
touched. The
`version` input must match `Cargo.toml`, both `Packager.toml` files, and
`package/macos/Info.plist` (the check-version action enforces this). AUR source
metadata is intentionally excluded because only the stable AUR publish job
renders its release version and hashes.

For a full dress rehearsal **including** `gh release create`: push the tag to
a private fork with the same variables and secrets. Prerelease tags
(`v*-rc.1` etc.) skip the Homebrew/Scoop/AUR/WinGet publish jobs even there,
but the GitHub release itself is created when `RELEASE_ENABLED=true` — on the
main repo that is public, so use the fork when "no publishing" is strict.

## Publish-workflow logic without CI

The manifest-rendering steps are plain shell and run locally:

```sh
# Scoop manifest rendering (publish-scoop.yml):
jq --arg version 9.9.9 --arg url http://example/x.zip --arg sha256 0000 \
  '.version = $version | .architecture."64bit".url = $url | .architecture."64bit".hash = $sha256' \
  package/scoop/clipboard-transformer.json

# Homebrew cask rendering (publish-homebrew.yml): run the ruby -e snippet
# against a copy of the cask with fake hashes and inspect the diff.
```

The shared pieces live in `.github/actions/`: `check-version`,
`install-cargo-packager` (pinned + cached), `fetch-release-sha256`, and
`commit-manifest`. Their `shell: bash` bodies can be executed locally
verbatim.
