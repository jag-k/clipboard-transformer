# Release and distribution

The tag-driven `.github/workflows/release.yml` workflow is the release
orchestrator. Cargo builds portable executables and archives, while
[`cargo-packager`](https://docs.crabnebula.dev/packager/) builds native desktop
bundles and installers. Small publishing workflows update Homebrew, Scoop, AUR,
and WinGet after the GitHub Release exists. To configure their credentials and
feature switches, see `ci-configuration.md`. To verify any of this without
publishing a release, see `packaging-verification.md`.

The project does not use cargo-dist. Its built-in Homebrew support produces a
CLI formula rather than the required `.app` cask, and its MSI packages CLI
binaries in a conventional `bin` directory rather than the desktop
application. Once both important packages require custom jobs, keeping its
generated release layer adds more configuration than it removes. The standard
cargo-packager WiX template does most of the Windows work. The inline fragment
in `Packager.toml` adds only project-specific registration and CLI PATH
components. `package/windows/verify-msi.ps1` installs and removes the result on
Windows CI instead of treating successful WiX linking as sufficient
verification.

## Release artifact matrix

| Platform | Artifact | Builder | Published by | Current status |
| --- | --- | --- | --- | --- |
| macOS arm64 | signed CLI `.tar.xz` | custom macOS job | GitHub release workflow | published |
| macOS x86_64 | signed CLI `.tar.xz` | custom macOS job | GitHub release workflow | published |
| macOS arm64/x86_64 | signed, notarized `.app.zip` | cargo-packager + custom macOS job | GitHub release workflow | published; app ticket stapled |
| macOS arm64/x86_64 | signed, notarized `.dmg` | cargo-packager + custom macOS job | GitHub release workflow | published; DMG ticket stapled |
| macOS arm64/x86_64 | Homebrew ZIP containing `.app` plus standalone CLI | custom macOS job | GitHub release workflow | published; contains the separately notarized CLI bytes |
| Homebrew | cask installing the macOS `.app` plus CLI, or the Linux x86_64 AppImage plus CLI | release workflow | `jag-k/homebrew-tap` | published; Linux install still needs validation |
| Flatpak remote | signed OSTree repository shared by current and future jag-k applications | release workflow | `jag-k/flatpak-repo` via GitHub Pages | prepared; enable after repository and signing-key bootstrap |
| Windows x86_64 | portable `clipboard-transformer-cli-<version>-x86_64.exe` CLI | Cargo/custom job | GitHub release workflow | published; unsigned |
| Windows x86_64 | portable `clipboard-transformer-app-<version>-x86_64.exe` GUI | Cargo/custom job | GitHub release workflow | published; unsigned |
| Windows x86_64 | portable ZIP containing GUI, CLI, and icon | Cargo/custom job | GitHub release workflow | published; unsigned |
| Windows x86_64 | `.msi` | cargo-packager + WiX fragment | GitHub release workflow | published; unsigned; corrected PATH upgrade ownership is pending the next release |
| Scoop | portable ZIP manifest | release workflow | `jag-k/scoop-bucket` | published |
| WinGet | MSI manifest | `wingetcreate` | custom WinGet job | bootstrap submission required |
| AUR | prebuilt and source-built package bases | release workflow | Arch User Repository | published |
| Linux | CLI archive, Homebrew AppImage + CLI bundle, AppImage, DEB, Pacman package + PKGBUILD, RPM | Cargo / cargo-packager / cargo-generate-rpm | GitHub release workflow | published; real-session validation remains incomplete |

The custom jobs explicitly package the public `clipboard-transformer` CLI and
the display-named desktop executable. The macOS job builds the CLI with default
features disabled, then signs it before creating its archive. The internal
`generate-schemas` tool and `clipboard-transformer-app` Cargo target must never
appear in CLI archives under internal names. Native package jobs build the app
with the `desktop` feature. This keeps tray, notification, hot-reload,
autostart, and host-loop dependencies out of the standalone CLI link graph.

## Homebrew

A generated CLI-only Homebrew **formula** would not install the macOS `.app`.
Clipboard Transformer therefore publishes one cross-platform custom cask from
`package/homebrew/clipboard-transformer.rb`.

The macOS packaging job produces a Homebrew-specific ZIP with two sibling
artifacts:

```text
Clipboard Transformer.app/
clipboard-transformer
```

On macOS, the cask installs the app and uses Homebrew's `binary` artifact for
the standalone CLI file. It deliberately does not link an executable from
inside the app bundle. On Linux x86_64, the same cask uses Homebrew's
`appimage` and `binary` artifacts to install a release bundle containing the
desktop AppImage and standalone CLI. This gives one installation command and
keeps the platform packages on exactly the same version:

```sh
brew install --cask jag-k/tap/clipboard-transformer
```

The previous source-built formula is intentionally retired once notarized DMGs
are published. It rebuilt a large Rust/Wasmtime application on every user's
machine and produced an ad-hoc-signed app instead of distributing the tested
release artifact.

## Windows catalogs

WinGet supports MSI, EXE, ZIP, and portable installer manifests. The community
repository requires a versioned HTTPS URL, a pinned SHA-256, unattended
installation, successful install/uninstall validation, and a manifest PR. A
code-signing certificate is not expressed as a mandatory field in the
community manifest schema, but unsigned public binaries have poor SmartScreen
reputation and may be blocked by managed environments. Windows signing remains
a recommended post-bootstrap improvement.

The first `JagK.ClipboardTransformer` manifest must be submitted and accepted
manually. Later stable releases are updated by
`.github/workflows/publish-winget.yml`; prereleases are skipped. The workflow
generates manifests locally, restores version-specific release notes and
version-pinned metadata that WingetCreate does not maintain itself, uploads the
exact submitted files as an Actions artifact, and then opens the package PR.

Do not bootstrap WinGet from the original `0.1.0` MSI. Its CLI is present, but
an upgrade can migrate the optional Environment feature after the conditional
PATH component has already been evaluated, leaving the CLI directory off the
machine `PATH`. The next MSI owns `CliPath` directly through the Environment
feature and is the first candidate suitable for the catalog.

Scoop publishes to the project-owned `jag-k/scoop-bucket`. The Windows artifact
job creates one immutable portable ZIP containing the GUI executable, CLI
executable, and icon. The manifest pins its SHA-256, creates a CLI shim, adds a
Start Menu shortcut, stops running processes before update/uninstall, preserves
application data, and includes `checkver`/`autoupdate` as a maintainer fallback.

Users register the bucket once and then install or update normally:

```powershell
scoop bucket add jag-k https://github.com/jag-k/scoop-bucket
scoop install jag-k/clipboard-transformer
scoop update clipboard-transformer
```

The release workflow updates `bucket/clipboard-transformer.json` after each
stable GitHub Release. Prereleases are not pushed to the bucket.

Chocolatey can distribute MSI, EXE, ZIP, or embedded binaries, but it adds a
separate `.nupkg`, PowerShell install scripts, API-key publishing, automated
verification, and community moderation. It overlaps heavily with WinGet and
Scoop, so it is a lower-priority channel rather than part of the first release.

## Linux

The Linux runtime now selects native `ext-data-control`/`wlr-data-control`
first and X11/XWayland with XFixes as fallback. The desktop requires a
StatusNotifierHost and actionable XDG portal notifications; it fails visibly
and exits instead of entering a degraded mode when any required capability is
missing.

`just package-linux` builds AppImage, DEB, Pacman archive plus `PKGBUILD`, and
RPM artifacts; `.github/workflows/build-linux-packages.yml` provides the same
matrix plus a portable CLI archive and is wired into tagged releases. Treat
the published artifacts as incompletely validated until installed packages
pass the platform checklist in real X11, GNOME XWayland-bridge, native
data-control Wayland, Xubuntu, and SteamOS sessions.

`.github/workflows/build-flatpak.yml` generates the Cargo source list from the
locked dependency graph, then independently builds the offline Flatpak
manifest and adds a single-file bundle to every Linux-enabled GitHub Release.
The bundle uses Flathub's Freedesktop runtime but the
application itself is not published on Flathub. For stable releases,
`.github/workflows/publish-flatpak.yml` can import that bundle into the shared,
signed `jag-k/flatpak-repo` remote and publish it through GitHub Pages. The
sandbox exposes X11,
Wayland, network access, the StatusNotifier watcher, and only the D-Bus names
needed by the current tray and notification activation implementation. It does
not expose host files or host executables, and autostart is disabled in-app.

For additional discovery, AppImageHub is the lowest-maintenance next channel:
it indexes the existing GitHub-hosted AppImage and does not add another build.
The project-owned `flake.nix` builds app and CLI outputs on Linux and macOS,
including a Darwin `.app` bundle. It can be installed directly from GitHub,
and successful project CI builds are pushed to the public `jag-k` Cachix
binary cache. An upstream nixpkgs submission is still useful after real NixOS
and macOS validation because that adds catalog discovery and official
binary-cache builds.

PPA, COPR, and OBS are build services rather than metadata-only catalogs like
AUR. They do not simply ingest the current binary DEB/RPM:

- Launchpad PPA accepts a signed Debian source upload and builds it for each
  selected Ubuntu series;
- COPR accepts an SRPM/spec or SCM repository containing a valid RPM spec and
  builds it in selected Fedora/RHEL-family chroots;
- OBS can reuse both Debian packaging and the RPM spec across several distro
  families, but requires the largest target/dependency matrix.

Start with COPR because the project already produces RPM metadata, add PPA
after a proper `debian/` source package exists, then reuse both recipes in OBS.
Vendor Cargo dependencies for deterministic remote builds and verify that each
builder can satisfy the workspace's declared Rust version. See
`linux-catalogs.md` for the concrete bootstrap and automation plan.

Do not automate or prepare a Flathub submission under the current policy. The
[inclusion policy](https://docs.flathub.org/docs/for-app-authors/requirements)
rejects tray-only applications and host system utilities, and its generative-AI
policy prohibits AI-assisted application content and submission materials
absent a reviewer-granted exception. The project can still test and publish
its own Flatpak bundle on GitHub. A Snap
would face similar host-access pressure and would likely require manually
approved classic confinement, so it remains lower priority than the native
artifacts already published.

## Required release configuration

The complete setup checklist is in
[CI and release configuration](ci-configuration.md). It lists every
GitHub Actions secret and variable, its minimum scope, where to obtain it, and
whether it is required for prerelease, stable, or optional catalog publishing.
Apple certificate creation, notarization credentials, and local verification
are covered in greater depth by
[macOS signing and notarization](macos-signing.md).
Windows Authenticode provider choices, signing order, and CI verification are
covered by [Windows signing](windows-signing.md).

When `MACOS_RELEASE_ENABLED=true`, the central preflight checks signing and
notarization configuration before starting a macOS runner. The macOS release
job does not publish an unsigned fallback as a normal release artifact. It
imports one Developer ID Application identity into an ephemeral keychain and
uses it for both the app bundle and standalone CLI.
The full identity string is derived from the imported `.p12`, so it cannot
drift from a separately configured GitHub variable.
cargo-packager signs the app and DMG, but its current built-in notarization
path only submits the app bundle. Apple also permits distributing an
unnotarized DMG containing an already notarized and stapled app. This project
instead submits the outermost DMG once, then staples the tickets issued for
both the DMG and its nested app; that gives the downloadable container an
offline ticket without adding another submission. The job submits a separate
temporary CLI-only ZIP because the standalone executable is not inside the
DMG. Archive formats and bare executables cannot carry a stapled ticket, so the
Homebrew ZIP and CLI `.tar.xz` contain the exact same signed CLI bytes covered
by that accepted submission.

## Release verification

Before pushing a release tag:

```sh
just package-macos
cargo fmt -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
git diff --check
```

`cargo release patch`, `cargo release minor`, or an explicit
`cargo release <version>` then previews the version replacements, release
commit, `v<version>` tag, and push configured in `release.toml`. After reviewing
the dry run, repeat with `--execute`. Publishing to crates.io is disabled; the
pushed tag starts the GitHub release workflow.

Before the dry run, curate the `Unreleased` section in `CHANGELOG.md`.
`cargo-release` turns it into a dated section for the new version and creates a
fresh `Unreleased` section. The GitHub workflow extracts the released section
and uses it as the GitHub Release description, so the changelog is the single
source of release notes rather than a second independently generated summary.

The first end-to-end release should use a prerelease tag and must be installed
on Apple Silicon macOS, Intel macOS, and Windows 11 from the downloaded GitHub
Release artifacts. Verify the app, CLI, upgrades, uninstall, autostart, a real
plugin invocation, Homebrew cask, and WinGet sandbox flow before publishing a
stable tag.
