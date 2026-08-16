# Packaging sources

This directory contains source manifests consumed by the release workflows.
They are not all directly installable from this repository.

| Source | Owner | Release-time values |
| --- | --- | --- |
| `homebrew/clipboard-transformer.rb` | Homebrew Cask DSL | The publish job replaces `version`, both macOS architecture hashes, and the Linux x86_64 AppImage + CLI bundle hash, then commits the rendered cask to `jag-k/homebrew-tap`. |
| `scoop/clipboard-transformer.json` | Scoop manifest JSON | The publish job replaces the concrete version, download URL, and hash with `jq`, then commits the rendered manifest to `jag-k/scoop-bucket`. |
| `aur/clipboard-transformer-bin/` and `aur/clipboard-transformer/` | Arch User Repository | The `-bin` package installs the prebuilt Linux release archive; the unsuffixed alternative builds both binaries from the immutable source archive. The stable-release job fills both hashes, regenerates both `.SRCINFO` files, and publishes the two independent AUR repositories with the 0BSD packaging license when enabled. |
| `flatpak/` | flatpak-builder | The manifest builds both binaries offline with a Cargo source list generated from `Cargo.lock` immediately before the build. Linux-enabled releases attach the resulting single-file bundle; stable releases can also update the shared, signed `jag-k/flatpak-repo` remote. It is not submitted to Flathub. |
| `../Packager.toml` and `windows/verify-msi.ps1` | cargo-packager / Windows Installer | The inline WiX fragment adds machine toast registration and owns the CLI PATH component through cargo-packager's Environment feature; the script verifies elevated silent install/uninstall, contents, PATH, shortcut, and registration. |
| `macos/Info.plist` and `macos/entitlements.plist` | cargo-packager | These are native package inputs, not text templates. |
| `linux/Packager.toml`, desktop entry, and D-Bus service | cargo-packager / cargo-generate-rpm | Build AppImage, DEB, Pacman/PKGBUILD, RPM, and the Homebrew AppImage + CLI bundle; native packages also install post-exit notification activation metadata. |
| `../flake.nix` and `../flake.lock` | Nix | Build the app and CLI on Linux and macOS. Darwin outputs also contain `Applications/Clipboard Transformer.app`; the GitHub-hosted flake uses the public `jag-k` Cachix binary cache and is the proving ground for a future human-authored nixpkgs submission. |

Published Homebrew and Scoop manifests intentionally contain a concrete
version and immutable release URL. Their `autoupdate` or repository publishing
workflow produces the next concrete revision; package managers do not install
from a floating "latest" URL.

`cargo release <level>` updates the shared release version in `Cargo.toml`,
`Packager.toml`, `package/linux/Packager.toml`, the macOS `Info.plist`, and the
Homebrew and Scoop source manifests according to `release.toml`. It does not
touch AUR metadata: the stable-release job renders both AUR packages from the
release version and immutable artifact hashes, while prereleases never reach
AUR. The release command also dates the current `Unreleased` section in
`CHANGELOG.md`; the tag workflow uses that section as the GitHub Release
description. It does not publish this application to crates.io. Run it without
`--execute` first to inspect the planned release.

The Scoop hooks preserve an enabled current-user autostart entry across
upgrades by refreshing it to Scoop's stable `current` path. Scoop invokes
uninstall hooks during updates too, so the cleanup hook distinguishes the
update path from a real uninstall before removing the Run and StartupApproved
registry values.

Structured files stay structured: Scoop is updated with `jq`, while the Cask
remains valid Ruby and WiX uses cargo-packager's built-in Handlebars renderer
plus a native fragment. Adding a second general-purpose template engine would
duplicate those native renderers without removing platform-specific packaging
behavior.

`just package-flatpak` generates the ignored `flatpak/cargo-sources.json` from
`Cargo.lock` before every build. `just gen-flatpak-sources` exposes that step
for diagnostics. The helper pins and verifies the official
`flatpak-cargo-generator`; CI never trusts an unversioned generator download.
`write-repository-descriptors.sh` renders the shared `.flatpakrepo` remote and
the application-specific `.flatpakref` with the exported repository public key.

## Publishing the shared Flatpak repository

The `jag-k/flatpak-repo` repository must be public, and GitHub Pages must
publish from the root of its `main` branch at `https://flatpak.jag-k.dev/`.
The Pages repository contains the matching `CNAME`; DNS must point the
`flatpak` CNAME to `jag-k.github.io`. The application release workflow needs a
dedicated cross-repository token and a dedicated, unencrypted signing key.
Neither secret is supplied by Flatpak or Flathub.

Create a fine-grained GitHub personal access token scoped only to
`jag-k/flatpak-repo`, with the repository permission `Contents: Read and
write`. Store its value in the `jag-k/clipboard-transformer` Actions secret
`FLATPAK_REPOSITORY_TOKEN`.

Generate a repository-specific GPG key. RSA is used here for compatibility
with older Flatpak clients:

```sh
gpg --batch --passphrase '' --quick-generate-key \
  'jag-k Flatpak Repository <flatpak@jag-k.dev>' rsa4096 sign 0

FLATPAK_KEY_FINGERPRINT="$(
  gpg --with-colons --list-secret-keys flatpak@jag-k.dev |
    awk -F: '$1 == "fpr" { print $10; exit }'
)"
```

After authenticating GitHub CLI with an account that administers the
application repository, install the three secrets:

```sh
printf '%s' "$FLATPAK_KEY_FINGERPRINT" |
  gh secret set FLATPAK_GPG_KEY_ID --repo jag-k/clipboard-transformer
gpg --armor --export-secret-keys "$FLATPAK_KEY_FINGERPRINT" |
  gh secret set FLATPAK_GPG_PRIVATE_KEY --repo jag-k/clipboard-transformer
gh secret set FLATPAK_REPOSITORY_TOKEN --repo jag-k/clipboard-transformer
```

The last command prompts for the fine-grained token. Enable the release jobs
with repository variables; all other release switches must also retain an
explicit `true` or `false` value:

```sh
gh variable set RELEASE_ENABLED --body true \
  --repo jag-k/clipboard-transformer
gh variable set LINUX_RELEASE_ENABLED --body true \
  --repo jag-k/clipboard-transformer
gh variable set FLATPAK_PUBLISH_ENABLED --body true \
  --repo jag-k/clipboard-transformer
```

The private key stays only in the application repository's Actions secrets.
The publication workflow exports the public key into `jag-k.flatpakrepo` and
signs the OSTree repository on every stable release. Generated
`cargo-sources.json` belongs to the application build and is intentionally not
stored in the binary `flatpak-repo` repository.
