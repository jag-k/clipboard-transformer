# CI and release configuration

This document is the setup checklist for the tag-driven GitHub Actions release
pipeline. It records every repository secret and variable consumed by the
workflows, what grants it carries, and where to obtain it.

Configure them in the Clipboard Transformer repository under **Settings →
Secrets and variables → Actions**. Put credentials under **Secrets** and
non-sensitive feature switches under **Variables**. Do not store credentials
in repository files, workflow inputs, or release tags.

For the complete Apple certificate and notarization setup, including local
verification and credential recovery limitations, see
[macOS signing and notarization](macos-signing.md).

## What a tag publishes

`.github/workflows/release.yml` runs for tags matching `v<version>`, where the
tag must exactly match the version in `Cargo.toml`. The workflow does not infer
which release parts are wanted: repository variables explicitly enable or
disable the release, each platform build, and each package catalog.

Set every switch to the literal lowercase value `true` or `false`. An absent,
empty, or differently spelled value is rejected instead of being treated as an
implicit default.

| Variable | Controls |
| --- | --- |
| `RELEASE_ENABLED` | Master switch. `false` validates the tag and then skips every build, GitHub Release, and catalog update. |
| `MACOS_RELEASE_ENABLED` | Signed and notarized macOS artifacts |
| `WINDOWS_RELEASE_ENABLED` | MSI and portable Windows artifacts |
| `LINUX_RELEASE_ENABLED` | Linux CLI archive, Flatpak bundle, Homebrew AppImage + CLI bundle, AppImage, `.deb`, Pacman package plus `PKGBUILD`, and `.rpm` artifacts |
| `HOMEBREW_PUBLISH_ENABLED` | Homebrew cask update for a stable release |
| `FLATPAK_PUBLISH_ENABLED` | Signed update to the shared `jag-k/flatpak-repo` repository for a stable release |
| `SCOOP_PUBLISH_ENABLED` | Scoop manifest update for a stable release |
| `AUR_PUBLISH_ENABLED` | AUR repository updates for a stable release |
| `WINGET_PUBLISH_ENABLED` | WinGet submission for a stable release |

When `RELEASE_ENABLED=false`, the other switches and release credentials are
not required. When it is `true`, all eight subordinate switches must be set
explicitly and at least one platform must be enabled.

The preflight job validates the complete configuration before starting any
platform runner:

- Homebrew requires the macOS build;
- Flatpak repository publication requires the Linux build;
- Scoop and WinGet require the Windows build;
- AUR requires the Linux build;
- an enabled macOS build requires all Apple signing and notarization secrets;
- an enabled catalog requires its publishing secret for stable tags;
- `CHANGELOG.md` must contain a non-empty section for the released version.
  The preflight job extracts the release notes and passes them to the release
  job as an artifact, so a missing section fails before any signing or
  notarization runner starts.

Before creating the public GitHub Release, the workflow also requires the full
Nix matrix to pass. For stable releases with Flatpak repository publication
enabled, it imports and signs the Flatpak build artifact before the GitHub
Release is created. A Nix build, Flatpak signature, or shared-repository update
failure therefore leaves the tagged run red without publishing a GitHub
Release.

Disabled platform jobs do not start, so disabling macOS also avoids consuming a
macOS runner and does not require Apple credentials. Prerelease tags build only
the enabled platforms, create a GitHub prerelease, and never update package
catalogs even if their switches are `true`.

## GitHub-provided credential

`GITHUB_TOKEN` is created automatically for each workflow run. Do not create a
secret with this name. The workflows grant it only the job-specific permissions
needed to read source, upload attestations, and create the GitHub Release.

## Nix binary cache

The `Nix` workflow builds the project flake on Linux, Apple Silicon macOS, and
Intel macOS. Determinate Nix no longer ships an installer for an Intel macOS
host, so the workflow uses a pinned upstream Nix installer on every runner and
builds `x86_64-darwin` natively on `macos-15-intel`. The workflow reads from the
public `jag-k` Cachix cache and pushes successful build outputs so users can
install matching flake revisions without compiling the application locally.

Create a per-cache token for the public `jag-k` cache with write permission and
store it as `CACHIX_AUTH_TOKEN`. The public cache URL and signing key belong in
`flake.nix`; the token belongs only in GitHub Actions secrets. Pull-request
runs use the cache read-only; pushes to `main`, tagged release gates, and manual
workflow runs publish successful outputs.

## macOS signing and notarization secrets

| Secret | Used for | Where it comes from |
| --- | --- | --- |
| `APPLE_CERTIFICATE` | Imports the Developer ID signing identity into an ephemeral CI keychain | Base64 of an exported Developer ID Application `.p12`; follow [the certificate and export procedure](macos-signing.md#1-create-the-developer-id-application-certificate) |
| `APPLE_CERTIFICATE_PASSWORD` | Unlocks the imported `.p12` | The password chosen when exporting the `.p12` |
| `APPLE_API_KEY_P8` | Authenticates `notarytool` submissions | The contents, or single-line base64, of an App Store Connect Team API `.p8` key |
| `APPLE_API_KEY_ID` | Selects the notarization API key | The Key ID shown beside that Team API key in App Store Connect |
| `APPLE_API_ISSUER_ID` | Selects the App Store Connect team | The Issuer ID shown on **Users and Access → Integrations → App Store Connect API** |

The `.p8` can be downloaded only once and the `.p12` private key cannot be
recovered from Apple. Keep both originals in a password manager in addition to
their GitHub secrets. See [macOS signing and notarization](macos-signing.md) for
the exact Apple portal paths, encoding commands, entitlements, and verification
steps.

## Homebrew tap

Create the `jag-k/homebrew-tap` repository before enabling Homebrew
publication. The workflow writes `Casks/clipboard-transformer.rb` to its
default branch.

Create a
[fine-grained GitHub personal access token](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens)
owned by an account allowed to push to that repository:

- repository access: only `jag-k/homebrew-tap`;
- repository permission: **Contents — Read and write**;
- use a finite expiration and rotate it before expiry.

Store it as `HOMEBREW_TAP_TOKEN`. A classic token with broader `repo` access
also works but is not preferred. Set `HOMEBREW_PUBLISH_ENABLED=true` only after
the tap and token have been tested. Homebrew publication also requires
`MACOS_RELEASE_ENABLED=true`.

## Scoop bucket

Create the `jag-k/scoop-bucket` repository before enabling Scoop publication.
The workflow writes `bucket/clipboard-transformer.json` to its default branch.

Create a
[fine-grained GitHub personal access token](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens)
with:

- repository access: only `jag-k/scoop-bucket`;
- repository permission: **Contents — Read and write**.

Store it as `SCOOP_BUCKET_TOKEN`, then set `SCOOP_PUBLISH_ENABLED=true`.
Keep it `false` until the bucket has been bootstrapped and the token has been
tested. Scoop publication also requires `WINDOWS_RELEASE_ENABLED=true`.

## Shared Flatpak repository

Create a public `jag-k/flatpak-repo` repository with an initial commit on its
default branch. Configure GitHub Pages to deploy from the root of that branch;
the resulting base URL must be `https://jag-k.github.io/flatpak-repo/`.
The repository is shared: future applications add refs to the same OSTree
repository and use the same `jag-k.flatpakrepo` remote descriptor.

Create a dedicated GPG signing key for this repository. A non-expiring primary
identity with a replaceable signing subkey is preferable; the CI signing key
must be usable non-interactively. Record its full fingerprint, keep an offline
backup and revocation certificate, and export only the CI signing material to
GitHub Actions. Store:

| Secret | Value |
| --- | --- |
| `FLATPAK_GPG_KEY_ID` | Full fingerprint of the repository signing key |
| `FLATPAK_GPG_PRIVATE_KEY` | ASCII-armored private key accepted by `gpg --import` |

Create a fine-grained GitHub personal access token scoped only to
`jag-k/flatpak-repo`, with **Contents — Read and write**, and store it as
`FLATPAK_REPOSITORY_TOKEN`.

The stable release workflow downloads the `.flatpak` build artifact before the
public GitHub Release is created, imports it into `repo/`, signs the application
ref and repository summary, generates static deltas, and commits
`jag-k.flatpakrepo` plus the application's `.flatpakref`. The manually
dispatched publication workflow instead downloads an existing GitHub Release
asset. Set `FLATPAK_PUBLISH_ENABLED=true` only after the repository, Pages
deployment, token, and signing-key backup have been verified. Publication also
requires `LINUX_RELEASE_ENABLED=true`.

## Arch User Repository

Create an [AUR account](https://aur.archlinux.org/register) and a dedicated SSH
key pair. For example:

```sh
ssh-keygen -t ed25519 -C clipboard-transformer-aur \
  -f clipboard-transformer-aur
```

Add `clipboard-transformer-aur.pub` to the AUR account under **My Account →
SSH Public Key**. Store the complete private key file
`clipboard-transformer-aur` as the GitHub secret `AUR_SSH_PRIVATE_KEY`.
Do not base64 it; GitHub Actions secrets accept the multiline OpenSSH value.

Bootstrap the `clipboard-transformer-bin` and `clipboard-transformer` AUR
repositories before enabling publication. The workflow renders stable versions
and hashes, regenerates `.SRCINFO`, and pushes `PKGBUILD`, `.SRCINFO`, and the
0BSD license for the AUR packaging files over SSH. AUR search keywords are
managed manually through the package-base web interface. The source-built
package continues to install the application's MPL-2.0 license from its release
archive. Follow the
[AUR submission guidelines](https://wiki.archlinux.org/title/AUR_submission_guidelines)
for the initial package repositories and account SSH setup.

Set `AUR_PUBLISH_ENABLED=true` only after both repository names and the key
have been verified. AUR publication also requires
`LINUX_RELEASE_ENABLED=true`. Prereleases are rejected both by the release
orchestrator and by `package/aur/update-package.sh`.

## WinGet

The first `JagK.ClipboardTransformer` version must be submitted manually with
`wingetcreate new` and accepted into `microsoft/winget-pkgs`. The automated job
can update an existing package id but does not bootstrap it.

Use the public, immutable MSI URL:

```powershell
winget install Microsoft.WingetCreate
wingetcreate new "https://github.com/jag-k/clipboard-transformer/releases/download/v<version>/clipboard-transformer-<version>-x86_64.msi"
```

Supply these required values when WinGetCreate cannot extract them:

```text
PackageIdentifier: JagK.ClipboardTransformer
PackageVersion: <version without v>
DefaultLocale: en-US
Publisher: jag-k
PackageName: Clipboard Transformer
License: MPL-2.0
ShortDescription: Rule-based clipboard transformer with a tray app and command-line interface
InstallerType: wix
Scope: machine
```

Also provide the repository homepage, issue tracker, versioned license URL,
inline release notes plus the release-notes URL, installation and configuration
documentation links, moniker `clipboard-transformer`, and relevant search tags.
Leave `PrivacyUrl` unset until the project has an explicit privacy-policy URL.
Do not submit an `Icons` block: WinGet validation refused it for this unsigned
package, and the accepted manifests carry no icon metadata.

Review the generated installer manifest and retain these verified fields:

- `Platform: [Windows.Desktop]` and the supported minimum Windows version;
- `Scope: machine`, all three MSI install modes, and
  `ElevationRequirement: elevationRequired`;
- `UpgradeBehavior: install` and command `clipboard-transformer`;
- the release date plus the MSI `ProductCode`;
- an Apps & Features entry matching the MSI's display name, publisher, product
  code, stable upgrade code, and installer type.

Leave `DisplayVersion` out of the Apps & Features entry. WinGet review rejects a
`DisplayVersion` equal to `PackageVersion`, and because WingetCreate refreshes
Apps & Features fields only where the existing manifest already populates them,
an omitted `DisplayVersion` stays omitted in every later update.

WinGetCreate extracts version-specific installer values, but it may not
preserve all curated metadata when bootstrapping a replacement manifest.
Recheck the inline release notes, documentation, installation note, and
installer fields before submission. Do not reuse a previous version's
installer hash or product code.

For later accepted versions, `publish-winget.yml` runs WingetCreate in two
phases. `update` first downloads the new MSI and refreshes the package version,
installer URL, SHA-256, ProductCode, and Apps & Features metadata. It clears
`ReleaseNotes`, `ReleaseNotesUrl` and the release date as version-specific
fields, and restores only the two the workflow passes explicitly:
`--release-notes-url` and `--release-date`, the latter taken from the GitHub
Release publication timestamp. The workflow then restores inline release notes
from the same validated changelog artifact used by the GitHub Release, advances
version-pinned license and documentation URLs, uploads the resulting manifests
for inspection, and only then submits them with `wingetcreate submit`. This
post-processing is required because WingetCreate deliberately clears inline
`ReleaseNotes` during a non-interactive update and does not advance
already-populated versioned metadata URLs.

WingetCreate branches from the upstream `master` HEAD and picks up curated
metadata from the newest version directory that is already merged, so a pull
request still under review is never used as a base and its manual fixes are not
inherited. Every fix that must survive belongs in this workflow. Each submission
creates a fresh `{packageId}-{version}-{guid}` branch, so WingetCreate cannot
recognise its own earlier submission; the workflow therefore skips before doing
any work when the version is already merged upstream, or when a branch matching
`JagK.ClipboardTransformer-<version>-` still has an open pull request. A skipped
submission is a successful job with a warning: to submit a rebuilt manifest for
a version under review, close that pull request first, then re-run the job.
Before
submitting, the workflow resets the fork's
`master` to upstream HEAD: WingetCreate otherwise syncs by merging, which fails
on a divergent fork, and a fork trailing upstream by too many commits cannot
have new references created in it at all.

Validate and sandbox-test the generated manifests as described in
`packaging-verification.md`; the first PR also requires accepting the Microsoft
CLA.

The official
[`wingetcreate` CI guidance](https://github.com/microsoft/winget-create)
documents a classic personal access token with `repo` scope. A fine-grained
token needs read and write on `Contents`, `Metadata` and `Pull requests` for the
`winget-pkgs` fork, plus read on this repository's `Contents` for the release
lookup. Fine-grained tokens only carry write access to repositories owned by the
selected owner, so if opening the pull request against `microsoft/winget-pkgs`
is rejected, fall back to a classic token with `public_repo`. Store the token as
`WINGET_CREATE_GITHUB_TOKEN`. It is used to create the manifest branch/fork and
submit a pull request to `microsoft/winget-pkgs`; it does not push to this
repository.

After the first manifest is accepted, set `WINGET_PUBLISH_ENABLED=true`. Keep it
`false` before then. WinGet publication also requires
`WINDOWS_RELEASE_ENABLED=true`.

## Configuration inventory

| Name | Kind | Required when |
| --- | --- | --- |
| `RELEASE_ENABLED` | Variable | Always; master `true`/`false` switch |
| `MACOS_RELEASE_ENABLED` | Variable | `RELEASE_ENABLED=true` |
| `WINDOWS_RELEASE_ENABLED` | Variable | `RELEASE_ENABLED=true` |
| `LINUX_RELEASE_ENABLED` | Variable | `RELEASE_ENABLED=true` |
| `HOMEBREW_PUBLISH_ENABLED` | Variable | `RELEASE_ENABLED=true` |
| `FLATPAK_PUBLISH_ENABLED` | Variable | `RELEASE_ENABLED=true` |
| `SCOOP_PUBLISH_ENABLED` | Variable | `RELEASE_ENABLED=true` |
| `AUR_PUBLISH_ENABLED` | Variable | `RELEASE_ENABLED=true` |
| `WINGET_PUBLISH_ENABLED` | Variable | `RELEASE_ENABLED=true` |
| `APPLE_CERTIFICATE` | Secret | `MACOS_RELEASE_ENABLED=true` |
| `APPLE_CERTIFICATE_PASSWORD` | Secret | `MACOS_RELEASE_ENABLED=true` |
| `APPLE_API_KEY_P8` | Secret | `MACOS_RELEASE_ENABLED=true` |
| `APPLE_API_KEY_ID` | Secret | `MACOS_RELEASE_ENABLED=true` |
| `APPLE_API_ISSUER_ID` | Secret | `MACOS_RELEASE_ENABLED=true` |
| `HOMEBREW_TAP_TOKEN` | Secret | Stable tag and `HOMEBREW_PUBLISH_ENABLED=true` |
| `FLATPAK_REPOSITORY_TOKEN` | Secret | Stable tag and `FLATPAK_PUBLISH_ENABLED=true` |
| `FLATPAK_GPG_KEY_ID` | Secret | Stable tag and `FLATPAK_PUBLISH_ENABLED=true` |
| `FLATPAK_GPG_PRIVATE_KEY` | Secret | Stable tag and `FLATPAK_PUBLISH_ENABLED=true` |
| `SCOOP_BUCKET_TOKEN` | Secret | Stable tag and `SCOOP_PUBLISH_ENABLED=true` |
| `AUR_SSH_PRIVATE_KEY` | Secret | Stable tag and `AUR_PUBLISH_ENABLED=true` |
| `WINGET_CREATE_GITHUB_TOKEN` | Secret | Stable tag and `WINGET_PUBLISH_ENABLED=true` |

For example, a Windows-only release with no catalog updates uses:

```text
RELEASE_ENABLED=true
MACOS_RELEASE_ENABLED=false
WINDOWS_RELEASE_ENABLED=true
LINUX_RELEASE_ENABLED=false
HOMEBREW_PUBLISH_ENABLED=false
FLATPAK_PUBLISH_ENABLED=false
SCOOP_PUBLISH_ENABLED=false
AUR_PUBLISH_ENABLED=false
WINGET_PUBLISH_ENABLED=false
```

This configuration needs no Apple or catalog credentials. To suspend tag
publishing entirely, change only `RELEASE_ENABLED=false`; the other values may
remain configured for the next release.

Before enabling a catalog, follow the non-publishing checks in
[packaging verification](packaging-verification.md). Use a prerelease tag in a
private fork for the first complete workflow rehearsal; a prerelease in the
public repository still creates a public GitHub Release.
