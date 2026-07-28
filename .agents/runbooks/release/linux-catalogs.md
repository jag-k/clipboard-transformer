# Linux catalog expansion

The project currently publishes native DEB, RPM, Pacman, AppImage, portable
CLI, AUR source and binary packages, plus a Homebrew cask that supports both
macOS and Linux. The direct DEB/RPM files are installable packages, not
automatically updating APT or DNF repositories.

Homebrew is one cross-platform channel:

```sh
brew install --cask jag-k/tap/clipboard-transformer
```

On macOS it installs the native app and CLI. On Linux x86_64 it installs the
AppImage and CLI.

## Why PPA, COPR, and OBS are more work than AUR

The AUR package bases contain recipes; users or AUR helpers perform the build,
while `clipboard-transformer-bin` downloads a prebuilt GitHub artifact.
Launchpad, COPR, and OBS are remote build services. They require source
packaging, build dependencies, supported target environments, repository
metadata, and publishing credentials.

The workspace currently declares Rust 1.91. Every selected remote build image
must provide a compatible compiler, or the source package must bootstrap an
allowed toolchain. Prefer a release source archive containing vendored Cargo
dependencies so builds do not depend on unrestricted network access or a
changing crates.io index.

## COPR first

[COPR](https://docs.pagure.org/copr.copr/user_documentation.html) accepts a
local spec/SRPM, a public URL, or an SCM repository containing a valid spec.
The existing binary RPM is not sufficient.

Add a maintained RPM spec that defines:

- source archive and vendored crates;
- build requirements and supported chroots;
- release-mode Cargo build for the desktop app and CLI;
- desktop file, icon, D-Bus activation metadata, licenses, and file ownership;
- upgrade and uninstall behavior.

Bootstrap:

1. Create the COPR project and enable selected Fedora chroots.
2. Build an SRPM locally and inspect it.
3. Submit it with `copr-cli build`, or configure SCM mode against a tagged
   repository and the spec path.
4. Install from the generated DNF repository on a real Fedora desktop.
5. Add a scoped COPR API token to CI only after the manual build works.

COPR is first because the project already has RPM packaging metadata and a
Fedora runtime validation target.

## Launchpad PPA second

[Launchpad PPA](https://documentation.ubuntu.com/launchpad/user/how-to/packaging/ppa-package-upload/)
accepts signed Debian source uploads and builds them itself; it does not accept
the existing binary DEB.

Add a proper `debian/` source package containing at least:

- `control`;
- `rules`;
- `changelog`;
- `copyright`;
- source format metadata;
- install manifests for the GUI, CLI, desktop file, icon, and D-Bus service.

Bootstrap:

1. Create a Launchpad account, register an OpenPGP key, and create the PPA.
2. Prepare an upstream source archive plus vendored crates.
3. Build and sign a source upload:

   ```sh
   debuild -S -sa -k'<Launchpad key>'
   ```

4. Upload the generated source changes:

   ```sh
   dput ppa:<launchpad-id>/clipboard-transformer \
     ../clipboard-transformer_*_source.changes
   ```

5. Use unique Debian versions for each supported Ubuntu series.
6. Install from the PPA on real X11 and Wayland/XWayland sessions before
   enabling release automation.

## OBS third

[Open Build Service](https://openbuildservice.org/files/manuals/obs-user-guide.pdf)
can reuse the Debian source packaging and RPM spec to produce repositories for
openSUSE, Fedora, Debian, Ubuntu, and related distributions.

OBS provides the broadest coverage but also the largest maintenance surface:
repository targets, distro-specific dependency names, signing, build results,
and runtime testing. Add it only after the COPR spec and PPA source package are
independently stable; do not create three divergent packaging recipes.

## Repology

Repology supplies the large packaging-status matrix:

```markdown
[![Packaging status](https://repology.org/badge/vertical-allrepos/clipboard-transformer.svg?exclude_unsupported=1)](https://repology.org/project/clipboard-transformer/versions)
```

It compares versions in repositories it indexes. Project-owned Homebrew taps,
Scoop buckets, and some personal PPA/COPR repositories may not appear, so it
cannot be the authoritative inventory of every Clipboard Transformer channel.
Keep the installation matrix in `docs/install.md` as the source of truth.

AUR keywords are package-base metadata managed manually in the AUR web UI. The
single input is whitespace-separated, not comma-separated:

```text
clipboard automation productivity text-processing desktop
```
