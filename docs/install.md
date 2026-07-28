# Installation

Clipboard Transformer ships a native desktop application and a separate
`clipboard-transformer` CLI.

> [!IMPORTANT]
> Clipboard Transformer is currently pre-release. No public GitHub Release,
> Homebrew cask, WinGet/Scoop manifest, or AUR package has been published.
> Build from source for now. The package names and commands later on this page
> document the distribution prepared for the first release.

## Build from source

Install the Rust toolchain declared in `rust-toolchain.toml` and
[`just`](https://github.com/casey/just):

```sh
git clone https://github.com/jag-k/clipboard-transformer.git
cd clipboard-transformer
just ci
just build-release
```

This writes the CLI and desktop executable under `target/release/`:

| Platform | CLI | Desktop executable |
| --- | --- | --- |
| macOS/Linux | `clipboard-transformer` | `clipboard-transformer-app` |
| Windows | `clipboard-transformer.exe` | `clipboard-transformer-app.exe` |

### macOS

Build a proper local app bundle and launch it:

```sh
just build-app
open 'target/macos/Clipboard Transformer.app'
```

`just install-app` can instead build and copy the local bundle into
`/Applications` after confirmation.

### Linux

Run `target/release/clipboard-transformer-app` from the graphical session.
Linux clipboard support depends on the active display session; see
[Linux desktop support](linux.md).

`just package-linux` builds local AppImage, DEB, Pacman, and RPM artifacts after
their additional packaging tools are installed.

### Windows

Run:

```powershell
target\release\clipboard-transformer-app.exe
```

`just package-windows-msi` builds a local MSI when cargo-packager and WiX are
available.

Native desktop and packaging dependencies are described in
[CONTRIBUTING.md](../CONTRIBUTING.md) and `just --list`.

## First run

Launch the desktop application. On every supported platform, it creates
`config.yaml` and `clipboard-transformer.schema.json` in the resolved config
directory when neither YAML nor TOML configuration already exists. No terminal
command is required.

The CLI is useful for optional inspection and validation:

```sh
clipboard-transformer paths
clipboard-transformer config check
```

Run `clipboard-transformer config init` only when you want to create the
starter YAML before launching the application. It never overwrites an existing
config.

## Distribution prepared for the first release

The following commands do not work until their corresponding public package or
release is published.

### Planned macOS distribution

The planned Homebrew command is:

```sh
brew install jag-k/tap/clipboard-transformer
```

The cask will install `Clipboard Transformer.app` and link the standalone CLI.

GitHub Releases are configured to provide both Apple Silicon and Intel builds:

- a DMG for normal manual installation;
- an `.app.zip`;
- a CLI-only `.tar.xz`;
- a combined Homebrew archive.

### Planned Windows distribution

The planned WinGet command is:

```powershell
winget install JagK.ClipboardTransformer
```

The planned Scoop commands are:

```powershell
scoop bucket add jag-k https://github.com/jag-k/scoop-bucket
scoop install clipboard-transformer
```

GitHub Releases are configured to provide an MSI, separate portable
executables, and a portable ZIP. The ZIP will contain:

- `Clipboard Transformer.exe` — the tray application;
- `clipboard-transformer.exe` — the console CLI.

The MSI will install the app, Start menu entry, toast activation support, and
optionally the CLI on `PATH`.

Windows artifacts are not code-signed yet. SmartScreen warnings and managed
environment restrictions remain possible after publication.

### Linux release packages

GitHub Releases are configured to provide:

- AppImage;
- DEB;
- RPM;
- Pacman archive and `PKGBUILD`;
- CLI archive.

Run `clipboard-transformer doctor` in the same graphical session before
enabling autostart.

### Arch User Repository

The repository prepares two AUR package bases:

| Package | Contents |
| --- | --- |
| `clipboard-transformer-bin` | Installs the precompiled `x86_64` GitHub Release archive. It does not need a Rust toolchain. |
| `clipboard-transformer` | Builds the desktop app and CLI from the release source archive with Cargo and runs the package tests. |

Neither package base is published yet. Once it is, the standard manual AUR
workflow will be:

```sh
git clone https://aur.archlinux.org/clipboard-transformer-bin.git
cd clipboard-transformer-bin
makepkg -si
```

Replace `clipboard-transformer-bin` with `clipboard-transformer` when you
prefer a local source build over the precompiled release archive.

Unofficial helpers are optional conveniences, not project requirements:

```sh
paru -S clipboard-transformer-bin
# or
yay -S clipboard-transformer-bin
```
