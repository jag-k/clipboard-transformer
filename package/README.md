# Packaging sources

This directory contains source manifests consumed by the release workflows.
They are not all directly installable from this repository.

| Source | Owner | Release-time values |
| --- | --- | --- |
| `homebrew/clipboard-transformer.rb` | Homebrew Cask DSL | The publish job replaces `version` and both architecture hashes, then commits the rendered cask to `jag-k/homebrew-tap`. |
| `scoop/clipboard-transformer.json` | Scoop manifest JSON | The publish job replaces the concrete version, download URL, and hash with `jq`, then commits the rendered manifest to `jag-k/scoop-bucket`. |
| `aur/clipboard-transformer-bin/` and `aur/clipboard-transformer/` | Arch User Repository | The `-bin` package installs the prebuilt Linux release archive; the unsuffixed alternative builds both binaries from the immutable source archive. The stable-release job fills both hashes, regenerates both `.SRCINFO` files, and publishes the two independent AUR repositories with the 0BSD packaging license when enabled. |
| `windows/extras.wxs` | cargo-packager | A small WiX fragment for the two project-specific additions: machine toast registration and the standalone CLI directory in PATH. The main MSI template remains cargo-packager's built-in template. |
| `macos/Info.plist` and `macos/entitlements.plist` | cargo-packager | These are native package inputs, not text templates. |
| `linux/Packager.toml`, desktop entry, and D-Bus service | cargo-packager / cargo-generate-rpm | Build AppImage, DEB, Pacman/PKGBUILD, and RPM artifacts with the desktop app, public CLI, and icon; native packages also install post-exit notification activation metadata. |

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
