# Linux catalog expansion

The project currently publishes native DEB, RPM, Pacman, AppImage and Flatpak
bundles, a portable CLI, AUR source and binary packages, plus a Homebrew cask
that supports both macOS and Linux. It also exposes a GitHub-hosted Nix flake.
The direct DEB/RPM/Flatpak files are installable packages, not automatically
updating APT, DNF, Flathub, or nixpkgs repositories.

Homebrew is one cross-platform channel:

```sh
brew install --cask jag-k/tap/clipboard-transformer
```

On macOS it installs the native app and CLI. On Linux x86_64 it installs the
AppImage and CLI.

## Flatpak and Flathub are separate

`package/flatpak/` is a real offline flatpak-builder source. The release
workflow produces an installable `.flatpak` bundle backed by the Flathub
Freedesktop runtime, but does not publish the application to Flathub. Stable
releases can instead update the shared, signed `jag-k/flatpak-repo` remote on
GitHub Pages. That repository may contain refs for multiple applications.

Current Flathub policy is a product gate, not a missing CI token: it rejects
tray-only applications and system utilities. For new submissions, its AI
policy also prohibits applications containing AI-generated or AI-assisted
code, documentation, or other content, as well as AI-generated submission
material. The policy is not retroactive for applications accepted before it
was introduced, and Flathub may grant exceptions to mature, well-maintained
projects. Do not open or automate a submission from this repository while
these restrictions apply. A separately
hosted Flatpak repository provides automatic updates without implying Flathub
acceptance or discovery in Flathub's catalog.

The ostree repository is stored inside a git repository, and git cannot track
empty directories. A fresh checkout of `jag-k/flatpak-repo` therefore loses
`repo/refs/mirrors`, `repo/refs/remotes`, `repo/state` and `repo/tmp`, and
`flatpak build-update-repo` then fails while listing refs. `publish-flatpak`
recreates that layout before touching the repository; keep the `mkdir -p` step
if the publishing script is reworked. Do not commit placeholder files under
`repo/refs/`, because ostree reads those directories as ref names.

## Nix and nixpkgs are separate

Users can install the project flake directly with:

```sh
nix profile install github:jag-k/clipboard-transformer
```

This supports Linux and macOS. Successful project CI builds are pushed to the
public `jag-k` Cachix binary cache, so matching installations download the
prebuilt output and only cache misses build locally. Nixpkgs is the community
package catalog shown by `search.nixos.org` and backed by the official cache;
it requires a separate upstream package contribution and review. Keep that
future contribution human-authored and test the project flake on NixOS and
both Darwin architectures first.

The flake declares the cache URL and public signing key. A multi-user Nix
daemon may reject client-specified substituters from an untrusted user; enable
the cache once with `nix run nixpkgs#cachix -- use jag-k` or declare the same
substituter and key through NixOS/nix-darwin system configuration.

Nixpkgs 26.05 is the final maintained release for `x86_64-darwin`; 26.11 drops
that platform. The project flake therefore pins only Intel macOS to
`nixpkgs-26.05-darwin` through its dedicated input while Linux and
`aarch64-darwin` follow the normal `nixos-26.05` input. Changing a local
`nix-channel` does not affect either input or `flake.lock`.

For the upstream contribution:

1. Publish and verify a stable source tag.
2. Add `pkgs/by-name/cl/clipboard-transformer/package.nix` in a Nixpkgs fork;
   the expression should fetch that immutable tag, build the CLI and desktop
   packages, install the Linux desktop metadata and Darwin `.app`, and carry
   complete `meta` including a maintainer.
3. Build with Nix sandboxing, run both binaries, launch the GUI on Linux and
   macOS, and run `nixpkgs-review wip` from the Nixpkgs checkout.
4. Submit a PR to `NixOS/nixpkgs` using its package checklist and respond to
   ofborg/reviewer results. Nixpkgs requires a responsible human to review the
   contribution and disclosure of non-trivial automation or LLM use.

The direct project-flake command remains useful before and after that PR, but
it does not create a nixpkgs catalog entry:

```sh
nix profile install github:jag-k/clipboard-transformer
```

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
Keep the platform sections in `docs/install.md` as the source of truth.

AUR keywords are package-base metadata managed manually in the AUR web UI. The
single input is whitespace-separated, not comma-separated:

```text
clipboard automation productivity text-processing desktop
```
