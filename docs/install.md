# Installation

Clipboard Transformer ships a native desktop application and a separate,
optional `clipboard-transformer` CLI. Download the
[latest stable release](https://github.com/jag-k/clipboard-transformer/releases/latest)
when you prefer a manual installation.

## Manual downloads (v0.1.1)

| Platform | Desktop app | Standalone CLI |
| --- | --- | --- |
| macOS, Apple Silicon | [DMG](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.1/clipboard-transformer-0.1.1-aarch64-apple-darwin.dmg) · [app ZIP](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.1/clipboard-transformer-0.1.1-aarch64-apple-darwin.app.zip) | [CLI archive](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.1/clipboard-transformer-cli-0.1.1-aarch64-apple-darwin.tar.xz) |
| macOS, Intel | [DMG](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.1/clipboard-transformer-0.1.1-x86_64-apple-darwin.dmg) · [app ZIP](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.1/clipboard-transformer-0.1.1-x86_64-apple-darwin.app.zip) | [CLI archive](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.1/clipboard-transformer-cli-0.1.1-x86_64-apple-darwin.tar.xz) |
| Windows, x86-64 | [MSI, app + CLI](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.1/clipboard-transformer-0.1.1-x86_64.msi) · [portable app + CLI](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.1/clipboard-transformer-0.1.1-x86_64-windows-portable.zip) · [app EXE](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.1/clipboard-transformer-app-0.1.1-x86_64.exe) | [CLI EXE](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.1/clipboard-transformer-cli-0.1.1-x86_64.exe) |
| Linux, x86-64 | [AppImage](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.1/clipboard-transformer-app_0.1.1_x86_64.AppImage) · [DEB](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.1/clipboard-transformer-app_0.1.1_amd64.deb) · [RPM](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.1/clipboard-transformer-0.1.1-1.x86_64.rpm) · [Pacman](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.1/clipboard-transformer-app_0.1.1_x86_64.tar.gz) | [CLI archive](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.1/clipboard-transformer-cli-0.1.1-x86_64-linux.tar.xz) |

These links target the tagged release directly. The release process updates the
version in this table and in the README before creating the next tag.

## Which download includes the CLI?

| Platform | Distribution | Desktop app | CLI |
| --- | --- | :---: | :---: |
| macOS | Homebrew cask | Yes | Yes |
| macOS | DMG or `.app.zip` | Yes | No |
| macOS | `clipboard-transformer-cli-…-apple-darwin.tar.xz` | No | Yes |
| Windows | MSI | Yes | Yes |
| Windows | Scoop or portable ZIP | Yes | Yes |
| Windows | `clipboard-transformer-app-….exe` | Yes | No |
| Windows | `clipboard-transformer-cli-….exe` | No | Yes |
| Linux | DEB, RPM, Pacman, AUR, or Homebrew | Yes | Yes |
| Linux | AppImage | Yes | No system CLI |
| Linux | `clipboard-transformer-cli-…-linux.tar.xz` | No | Yes |

The desktop application does not require the CLI. Install the CLI only for
terminal pipelines, explicit clipboard commands, or diagnostics.

## macOS

### Homebrew

Homebrew installs both `Clipboard Transformer.app` and the standalone CLI:

```sh
brew install --cask jag-k/tap/clipboard-transformer
```

### Manual app installation

From the
[latest release](https://github.com/jag-k/clipboard-transformer/releases/latest),
download the DMG matching the Mac:

- `aarch64-apple-darwin.dmg` for Apple Silicon;
- `x86_64-apple-darwin.dmg` for an Intel Mac.

Open the DMG and copy **Clipboard Transformer** to **Applications**. The
`.app.zip` is an alternative app-only download.

The DMG and `.app.zip` do not contain the standalone CLI. To install it
manually, download the matching
`clipboard-transformer-cli-…-apple-darwin.tar.xz`, extract it, and place the
`clipboard-transformer` file on `PATH`, for example:

```sh
sudo install -d /usr/local/bin
sudo install -m 0755 clipboard-transformer /usr/local/bin/clipboard-transformer
```

## Windows

### MSI

Download `clipboard-transformer-<version>-x86_64.msi` from the
[latest release](https://github.com/jag-k/clipboard-transformer/releases/latest).
The MSI installs the tray application, Start menu entry, toast activation
support, and the standalone CLI. Its PATH option makes
`clipboard-transformer.exe` available in new terminal sessions.
The MSI is a per-machine installation, so Windows requests administrator
approval. Manual non-interactive installation must be launched from an
elevated process.

> [!NOTE]
> An upgrade with the original `0.1.0` MSI can leave the installed CLI
> directory off `PATH`. The CLI remains available at
> `C:\Program Files\Clipboard Transformer\bin\clipboard-transformer.exe`.
> Feature ownership is corrected for the next release.

The Windows artifacts are not Authenticode-signed yet. Windows SmartScreen or
managed-device policy may warn about or block them even though release
artifacts include GitHub attestations and SHA-256 files.

### Scoop

Scoop installs the portable app and exposes the CLI through its shim:

```powershell
scoop bucket add jag-k https://github.com/jag-k/scoop-bucket
scoop install clipboard-transformer
```

### Portable files

The `clipboard-transformer-<version>-x86_64-windows-portable.zip` download
contains both:

- `Clipboard Transformer.exe` — the tray application;
- `clipboard-transformer.exe` — the console CLI.

Separate GUI-only and CLI-only `.exe` downloads are also available. Portable
downloads do not create an installer-owned Start menu entry or add the CLI
directory to `PATH`.

WinGet publication is pending its first manifest submission. After
`JagK.ClipboardTransformer` is accepted into the community catalog, stable
release updates are prepared to publish automatically.

## Linux

Linux clipboard support depends on the active display session. Read
[Linux desktop support](linux.md) and run `clipboard-transformer doctor` in the
same graphical session before enabling autostart.

### Homebrew on Linux

The project cask installs the AppImage and standalone CLI together:

```sh
brew install --cask jag-k/tap/clipboard-transformer
```

Native distro packages are generally a better fit when one is available.

### Arch User Repository

The prebuilt package is the shortest AUR installation:

```sh
git clone https://aur.archlinux.org/clipboard-transformer-bin.git
cd clipboard-transformer-bin
makepkg -si
```

`clipboard-transformer-bin` installs the precompiled x86_64 release. The
`clipboard-transformer` package instead builds the desktop app and CLI from
source with Cargo.

Unofficial AUR helpers are optional:

```sh
paru -S clipboard-transformer-bin
# or
yay -S clipboard-transformer-bin
```

### DEB, RPM, and Pacman packages

The native release packages install both the desktop application and the CLI,
plus the desktop entry, icon, and D-Bus activation metadata:

```sh
# Debian or Ubuntu
sudo apt install ./clipboard-transformer-app_<version>_amd64.deb

# Fedora or another RPM-based distribution
sudo dnf install ./clipboard-transformer-<version>-1.x86_64.rpm

# Arch Linux without AUR
sudo pacman -U ./clipboard-transformer-app_<version>_x86_64.tar.gz
```

These packages are direct GitHub downloads, not an APT, DNF, or Pacman
repository. The package manager can uninstall them, but it will not discover a
new Clipboard Transformer release automatically.

### AppImage and manual CLI

The AppImage is a portable desktop application:

```sh
chmod +x clipboard-transformer-app_<version>_x86_64.AppImage
./clipboard-transformer-app_<version>_x86_64.AppImage
```

It does not install desktop integration, D-Bus activation metadata, or a
system CLI. Download `clipboard-transformer-cli-…-x86_64-linux.tar.xz`
separately when the CLI is needed:

```sh
mkdir -p ~/.local/bin
install -m 0755 clipboard-transformer ~/.local/bin/clipboard-transformer
```

Make sure `~/.local/bin` is on `PATH`.

### Package version status

[Repology](https://repology.org/project/clipboard-transformer/versions)
compares versions found in the repositories it indexes. Project-owned
Homebrew taps, Scoop buckets, and some personal PPA/COPR repositories may not
appear there, so the matrix supplements rather than replaces the installation
table above.

## Verify a manual download

Every release asset has an adjacent `.sha256` file. Download both files, keep
them in the same directory, and verify before installation:

```sh
# Linux
sha256sum --check <artifact>.sha256

# macOS
shasum -a 256 --check <artifact>.sha256
```

On Windows, run `Get-FileHash <artifact> -Algorithm SHA256` and compare the
result with the adjacent `.sha256` file.

GitHub also publishes build provenance for the release artifacts. With the
GitHub CLI installed:

```sh
gh attestation verify <artifact> --repo jag-k/clipboard-transformer
```

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

Native desktop and packaging dependencies are described in
[CONTRIBUTING.md](../CONTRIBUTING.md) and `just --list`.
