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
| `LINUX_RELEASE_ENABLED` | Linux CLI archive, Homebrew AppImage + CLI bundle, AppImage, `.deb`, Pacman package plus `PKGBUILD`, and `.rpm` artifacts |
| `HOMEBREW_PUBLISH_ENABLED` | Homebrew cask update for a stable release |
| `SCOOP_PUBLISH_ENABLED` | Scoop manifest update for a stable release |
| `AUR_PUBLISH_ENABLED` | AUR repository updates for a stable release |
| `WINGET_PUBLISH_ENABLED` | WinGet submission for a stable release |

When `RELEASE_ENABLED=false`, the other switches and release credentials are
not required. When it is `true`, all seven subordinate switches must be set
explicitly and at least one platform must be enabled.

The preflight job validates the complete configuration before starting any
platform runner:

- Homebrew requires the macOS build;
- Scoop and WinGet require the Windows build;
- AUR requires the Linux build;
- an enabled macOS build requires all Apple signing and notarization secrets;
- an enabled catalog requires its publishing secret for stable tags;
- `CHANGELOG.md` must contain a non-empty section for the released version.
  The preflight job extracts the release notes and passes them to the release
  job as an artifact, so a missing section fails before any signing or
  notarization runner starts.

Disabled platform jobs do not start, so disabling macOS also avoids consuming a
macOS runner and does not require Apple credentials. Prerelease tags build only
the enabled platforms, create a GitHub prerelease, and never update package
catalogs even if their switches are `true`.

## GitHub-provided credential

`GITHUB_TOKEN` is created automatically for each workflow run. Do not create a
secret with this name. The workflows grant it only the job-specific permissions
needed to read source, upload attestations, and create the GitHub Release.

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
documentation links, the immutable versioned Windows icon with its SHA-256,
moniker `clipboard-transformer`, and relevant search tags. Leave `PrivacyUrl`
unset until the project has an explicit privacy-policy URL.

Review the generated installer manifest and retain these verified fields:

- `Platform: [Windows.Desktop]` and the supported minimum Windows version;
- `Scope: machine`, all three MSI install modes, and
  `ElevationRequirement: elevationRequired`;
- `UpgradeBehavior: install` and command `clipboard-transformer`;
- the release date plus the MSI `ProductCode`;
- an Apps & Features entry matching the MSI's display name, publisher,
  display version, product code, stable upgrade code, and installer type.

WinGetCreate extracts version-specific installer values, but it may not
preserve all curated metadata when bootstrapping a replacement manifest.
Recheck the inline release notes, icon, documentation, installation note, and
installer fields before submission. Do not reuse a previous version's
installer hash or product code.

For later accepted versions, `publish-winget.yml` runs WingetCreate in two
phases. `update` first downloads the new MSI and refreshes the package version,
installer URL, SHA-256, ProductCode, Apps & Features metadata, release date, and
release-notes URL. The workflow then restores inline release notes from the
same validated changelog artifact used by the GitHub Release, advances
version-pinned license/documentation/icon URLs, recalculates the icon SHA-256,
uploads the resulting manifests for inspection, and only then submits them
with `wingetcreate submit`. This post-processing is required because
WingetCreate deliberately clears inline `ReleaseNotes` during a non-interactive
update and does not advance already-populated versioned metadata URLs.

Validate and sandbox-test the generated manifests as described in
`packaging-verification.md`; the first PR also requires accepting the Microsoft
CLA.

The official
[`wingetcreate` CI guidance](https://github.com/microsoft/winget-create)
uses a GitHub personal access token with classic `repo` scope. Store that token as
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
| `SCOOP_PUBLISH_ENABLED` | Variable | `RELEASE_ENABLED=true` |
| `AUR_PUBLISH_ENABLED` | Variable | `RELEASE_ENABLED=true` |
| `WINGET_PUBLISH_ENABLED` | Variable | `RELEASE_ENABLED=true` |
| `APPLE_CERTIFICATE` | Secret | `MACOS_RELEASE_ENABLED=true` |
| `APPLE_CERTIFICATE_PASSWORD` | Secret | `MACOS_RELEASE_ENABLED=true` |
| `APPLE_API_KEY_P8` | Secret | `MACOS_RELEASE_ENABLED=true` |
| `APPLE_API_KEY_ID` | Secret | `MACOS_RELEASE_ENABLED=true` |
| `APPLE_API_ISSUER_ID` | Secret | `MACOS_RELEASE_ENABLED=true` |
| `HOMEBREW_TAP_TOKEN` | Secret | Stable tag and `HOMEBREW_PUBLISH_ENABLED=true` |
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
