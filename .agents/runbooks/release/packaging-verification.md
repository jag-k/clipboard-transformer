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
| Homebrew cask | `package/homebrew/clipboard-transformer.rb` + `publish-homebrew.yml` | macOS, locally |
| Windows MSI | `build-windows-msi.yml` / `just`-equivalents (cargo-packager + WiX) | Windows VM or CI artifact |
| Windows portable ZIP | `build-windows-msi.yml` staging step | Windows VM |
| Scoop manifest | `package/scoop/clipboard-transformer.json` + `publish-scoop.yml` | Windows VM |
| WinGet manifest | `publish-winget.yml` (wingetcreate against the MSI) | Windows VM |
| Linux AppImage, DEB, Pacman/PKGBUILD | `just package-linux` / `build-linux-packages.yml` | matching Linux VM |
| Linux RPM | `just package-linux-rpm` / `build-linux-packages.yml` | Fedora/openSUSE VM |
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

The cask installs from the release `-homebrew.zip` (the `.app` plus the
standalone CLI). To test without a release:

```sh
# 1. Build the zip the way CI does (app + CLI + LICENSE in one folder):
just package-app
mkdir -p target/homebrew-root
ditto "target/packager/Clipboard Transformer.app" "target/homebrew-root/Clipboard Transformer.app"
cp target/release/clipboard-transformer LICENSE target/homebrew-root/
ditto -c -k --sequesterRsrc target/homebrew-root target/homebrew-local.zip

# 2. Point a copy of the cask at it:
cp package/homebrew/clipboard-transformer.rb /tmp/clipboard-transformer.rb
shasum -a 256 target/homebrew-local.zip   # replace both sha256 values
# replace the url line with: url "file://#{Dir.pwd}/target/homebrew-local.zip"

# 3. Lint and install:
brew style /tmp/clipboard-transformer.rb
brew audit --cask /tmp/clipboard-transformer.rb || true   # audit needs a tap context; style is the hard gate
brew install --cask /tmp/clipboard-transformer.rb
```

The `ruby` rendering snippet from `publish-homebrew.yml` can be run directly
in a shell with fake sha256 values to preview the published cask.

## Windows: VM or real machine

Use any Windows 11 machine or the free Microsoft dev VM (runs under
UTM/Parallels/VMware on a Mac). Artifacts come from a CI dry run (below) or
are built on the VM with the same commands as
`.github/workflows/build-windows-msi.yml`.

**MSI:**

```powershell
msiexec /i clipboard-transformer-<version>-x86_64.msi /l*v install.log
```

Then check: Start Menu entry launches the tray app; an actionable
notification's buttons work (requires the ToastActivatorCLSID registration);
`clipboard-transformer.exe --help` from the install dir; uninstall removes the
Start Menu entry and HKLM CLSID registration.

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
winget settings --enable LocalManifestFiles
wingetcreate update JagK.ClipboardTransformer --urls <msi url or path> --version <version>  # writes manifests locally without --submit
winget validate --manifest <generated manifest dir>
winget install --manifest <generated manifest dir>
```

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

## CI without publishing

All three build workflows accept `workflow_dispatch` in addition to
`workflow_call`, so each one runs on its own without the release job and without
a temporary wrapper workflow:

```sh
gh workflow run build-macos-packages.yml -f version=0.1.0
gh workflow run build-windows-msi.yml -f version=0.1.0
gh workflow run build-linux-packages.yml -f version=0.1.0
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
