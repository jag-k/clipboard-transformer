# Installation

Clipboard Transformer ships a native desktop application and a separate,
optional `clipboard-transformer` CLI. The desktop application does not require
the CLI; install it only for terminal pipelines, explicit clipboard commands,
or diagnostics.

Choose the operating system first. Package managers and update-capable methods
come before direct release downloads in each section.
Direct links below target **v0.1.3**.

## macOS

### Homebrew

Homebrew installs both `Clipboard Transformer.app` and the standalone CLI:

```sh
brew install --cask jag-k/tap/clipboard-transformer
```

### Nix

The project flake supports Apple Silicon and Intel macOS. It follows Nixpkgs
26.05 on Apple Silicon and pins Intel macOS to the maintained
`nixpkgs-26.05-darwin` branch, because Nixpkgs 26.11 drops `x86_64-darwin`.
Nixpkgs plans to maintain the Intel branch through the end of 2026. The flake
provides both the menu bar application and CLI:

```sh
nix profile install github:jag-k/clipboard-transformer
nix run github:jag-k/clipboard-transformer
nix run github:jag-k/clipboard-transformer#cli -- --version
```

The Nix output includes `Applications/Clipboard Transformer.app`. It is not in
nixpkgs yet, but successful project CI builds are published to the public
`jag-k` Cachix binary cache declared by the flake. Nix downloads a matching
prebuilt output when one is available and falls back to a source build when it
is not.

Multi-user Nix installations may require enabling the cache once at the daemon
level before installation:

```sh
nix run nixpkgs#cachix -- use jag-k
```

`nix run github:jag-k/clipboard-transformer` opens that application bundle
with macOS `open` and returns control to the terminal. `nix profile install`
keeps the bundle under `~/.nix-profile/Applications`; vanilla Nix does not copy
it into `/Applications`. Tools such as `mac-app-util`, nix-darwin, or Home
Manager can expose the immutable bundle through managed application symlinks.

### Direct downloads

| Architecture | Desktop application | Standalone CLI |
| --- | --- | --- |
| Apple Silicon | [DMG](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.3/clipboard-transformer-0.1.3-aarch64-apple-darwin.dmg) · [app ZIP](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.3/clipboard-transformer-0.1.3-aarch64-apple-darwin.app.zip) | [CLI archive](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.3/clipboard-transformer-cli-0.1.3-aarch64-apple-darwin.tar.xz) |
| Intel | [DMG](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.3/clipboard-transformer-0.1.3-x86_64-apple-darwin.dmg) · [app ZIP](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.3/clipboard-transformer-0.1.3-x86_64-apple-darwin.app.zip) | [CLI archive](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.3/clipboard-transformer-cli-0.1.3-x86_64-apple-darwin.tar.xz) |

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

### Scoop

Scoop installs the portable app and exposes the CLI through its shim:

```powershell
scoop bucket add jag-k https://github.com/jag-k/scoop-bucket
scoop install clipboard-transformer
```

### WinGet

> [!WARNING]
> Clipboard Transformer is not in the WinGet community catalog yet. The command
> below will work only after
> [microsoft/winget-pkgs#411013](https://github.com/microsoft/winget-pkgs/pull/411013)
> is merged and the catalog update becomes available.

```powershell
winget install --exact --id JagK.ClipboardTransformer
```

After the first manifest is accepted, stable release updates are prepared to
publish automatically.

### Direct downloads

All current Windows builds target x86-64.

| Format | Contents | Download |
| --- | --- | --- |
| MSI installer | Desktop app + CLI | [MSI](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.3/clipboard-transformer-0.1.3-x86_64.msi) |
| Portable ZIP | Desktop app + CLI | [ZIP](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.3/clipboard-transformer-0.1.3-x86_64-windows-portable.zip) |
| Portable GUI | Desktop app only | [EXE](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.3/clipboard-transformer-app-0.1.3-x86_64.exe) |
| Portable CLI | CLI only | [EXE](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.3/clipboard-transformer-cli-0.1.3-x86_64.exe) |

The MSI installs the tray application, Start menu entry, toast activation
support, and the standalone CLI. Its PATH option makes
`clipboard-transformer.exe` available in new terminal sessions. It is a
per-machine installation, so Windows requests administrator approval.

> [!NOTE]
> An upgrade with the original `0.1.0` MSI can leave
> the installed CLI directory off `PATH`. The CLI remains available at
> `C:\Program Files\Clipboard Transformer\bin\clipboard-transformer.exe`.
> Feature ownership is corrected for the next release.

The `clipboard-transformer-<version>-x86_64-windows-portable.zip` download
contains both:

- `Clipboard Transformer.exe` — the tray application;
- `clipboard-transformer.exe` — the console CLI.

Separate GUI-only and CLI-only `.exe` downloads are also available. Portable
downloads do not create an installer-owned Start menu entry or add the CLI
directory to `PATH`.

The Windows artifacts are not Authenticode-signed yet. Windows SmartScreen or
managed-device policy may warn about or block them even though release
artifacts include GitHub attestations and SHA-256 files.

## Linux

Linux clipboard support depends on the active display session. Read
[Linux desktop support](linux.md) and run `clipboard-transformer doctor` in the
same graphical session before enabling autostart.

### Testing disclaimer

I do not have a general-purpose Linux desktop machine on which I can test every
installation method across different distributions, desktop environments, and
display-session configurations. The Linux hardware available to me is limited
to a couple of headless servers and a Steam Deck, which I no longer use as an
installation test bed because experimenting on it is impractical.

If you encounter a problem installing or launching Clipboard Transformer, or
find that the application does not work as documented, please
[open an issue](https://github.com/jag-k/clipboard-transformer/issues). Reports
from real Linux setups are especially helpful.

### Flatpak repository

The shared `jag-k` Flatpak repository is the update-capable Flatpak route and
may contain multiple applications. Once published, add it and install Clipboard
Transformer with:

```sh
flatpak remote-add --user --if-not-exists jag-k \
  https://flatpak.jag-k.dev/jag-k.flatpakrepo
flatpak install --user jag-k dev.jag_k.clipboard_transformer
```

The application-specific `.flatpakref` performs both operations in one step:

```sh
flatpak install --user \
  https://flatpak.jag-k.dev/clipboard-transformer.flatpakref
```

Updates then arrive through the normal `flatpak update` flow. Until that URL
is published, use a release bundle from the direct-download section below.

The Flatpak contains the desktop app and a sandboxed CLI. Configuration is
stored below `~/.var/app/dev.jag_k.clipboard_transformer/config/`. Host
executables and arbitrary host files are not visible to `shell` rules, and
in-app autostart is disabled. URL imports and plugin downloads remain available
through the sandbox's network permission.

Launch the desktop application normally:

```sh
flatpak run dev.jag_k.clipboard_transformer
```

The CLI is included in the same Flatpak but is not added to the host `PATH`.
Select it explicitly with `--command` and pass CLI arguments after the app ID:

```sh
flatpak run --command=clipboard-transformer \
  dev.jag_k.clipboard_transformer --help
flatpak run --command=clipboard-transformer \
  dev.jag_k.clipboard_transformer doctor
```

### Nix on Linux

The same project flake supports `x86_64-linux` and `aarch64-linux`:

```sh
nix profile install github:jag-k/clipboard-transformer
nix run github:jag-k/clipboard-transformer
```

This is direct installation from the project flake, not a nixpkgs catalog
entry. Successful project CI builds are available from the public `jag-k`
Cachix binary cache declared by the flake; Nix falls back to a local source
build when that cache does not contain the exact requested output.

If a multi-user Nix daemon ignores the cache settings declared by the flake,
enable the cache once before installation:

```sh
nix run nixpkgs#cachix -- use jag-k
```

The package places its executables in the Nix profile and desktop metadata
under the profile's `share/applications`. NixOS and Home Manager integrate
those profile paths with the desktop environment; standalone Nix does not copy
them into `/usr`.

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

### Homebrew on Linux

The project cask installs the AppImage and standalone CLI together:

```sh
brew install --cask jag-k/tap/clipboard-transformer
```

Native distro packages are generally a better fit when one is available.

### Package version status

[Repology](https://repology.org/project/clipboard-transformer/versions)
compares versions found in the repositories it indexes. Project-owned
Homebrew taps, Scoop buckets, and some personal PPA/COPR repositories may not
appear there.

### Direct downloads

The current direct Linux artifacts target x86-64.

| Format | Contents | Download |
| --- | --- | --- |
| AppImage | Desktop app only | [AppImage](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.3/clipboard-transformer-app_0.1.3_x86_64.AppImage) |
| Debian package | Desktop app + CLI | [DEB](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.3/clipboard-transformer-app_0.1.3_amd64.deb) |
| RPM package | Desktop app + CLI | [RPM](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.3/clipboard-transformer-0.1.3-1.x86_64.rpm) |
| Pacman package | Desktop app + CLI | [archive](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.3/clipboard-transformer-app_0.1.3_x86_64.tar.gz) |
| CLI archive | CLI only | [archive](https://github.com/jag-k/clipboard-transformer/releases/download/v0.1.3/clipboard-transformer-cli-0.1.3-x86_64-linux.tar.xz) |

Flatpak-enabled releases also attach
`clipboard-transformer-<version>-x86_64.flatpak` to the matching
[GitHub release](https://github.com/jag-k/clipboard-transformer/releases).
Install and launch that bundle with:

```sh
flatpak install --user ./clipboard-transformer-<version>-x86_64.flatpak
flatpak run dev.jag_k.clipboard_transformer
```

Its sandboxed CLI can be invoked explicitly:

```sh
flatpak run --command=clipboard-transformer \
  dev.jag_k.clipboard_transformer doctor
```

The native DEB, RPM, and Pacman packages install the desktop application, CLI,
desktop entry, icon, and D-Bus activation metadata:

```sh
# Debian or Ubuntu
sudo apt install ./clipboard-transformer-app_<version>_amd64.deb

# Fedora or another RPM-based distribution
sudo dnf install ./clipboard-transformer-<version>-1.x86_64.rpm

# Arch Linux without AUR
sudo pacman -U ./clipboard-transformer-app_<version>_x86_64.tar.gz
```

These files are not an APT, DNF, or Pacman repository. The package manager can
uninstall them, but it will not discover new releases automatically.

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
