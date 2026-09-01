# Changelog

All notable changes to Clipboard Transformer are documented in this file.

The format is based on [Keep a Changelog], and this project follows
[Semantic Versioning].

<!-- next-header -->
## [Unreleased] - ReleaseDate

## [0.1.6] - 2026-09-01

### Added

- **Rule groups:** (#29) Add `groups` and `group_imports` to the configuration, a
  versioned `<state_dir>/groups.json` state document, tray switches for
  rule-used groups with rule counts and descriptions, and `groups list` /
  `enable` / `disable` CLI commands. Group membership is inherited from
  rulesets and import edges; ignored groups and explicit group state disable
  the rules that depend on them. Short-form import edges accept `groups` and
  `ignore_imported_groups` as sibling keys, and group state may be written for
  an id before any rule uses it.

### Changed

- **Dependencies:** Update `clap`, `zbus`, `timeago`, `schemars`, `uuid`,
  `log`, `futures-util`, and `ureq`, and drop the runtime's unused
  `thiserror` dependency. Keep the plugin-author and runtime configuration
  schemas on draft-07 and byte-compatible across the Schemars migration.
- **Packaging/Nix:** Refresh the pinned `cachix/install-nix-action` v31
  revision.

### Fixed

- **Developer tooling:** Point `just gen-config-schema` at the current
  `config schema` CLI command.
- **CI:** Keep tray icon code generation clean under the Rust 1.98 Clippy
  slice-chunking lints.
- **Release:** Keep `cargo-release` from pushing the release commit and tag
  together so required main-branch checks can finish before publication.
- **Linux/Tray:** Detect Flatpak once in the runtime host and pass the sandbox
  state into the tray backend instead of sampling the environment twice.

## [0.1.5] - 2026-08-16

### Changed

- **Release:** Require the Nix matrix and enabled stable Flatpak repository
  publication to succeed before creating the public GitHub Release. Keep the
  release commit and tag pushes separate so main-branch checks can finish
  before the tag starts publication.

### Fixed

- **Packaging/Flatpak:** Pass the target architecture through `build-sign`'s
  `--arch` option instead of as an invalid positional argument.
- **Packaging/Nix:** Create the Linux D-Bus service destination before
  substitution, and use pinned upstream Nix to build the Intel macOS output on
  a native Intel runner now that Determinate no longer ships an Intel-host
  installer.

## [0.1.4] - 2026-08-16

### Added

- **Desktop/Notifications:** Add independent `config.notifications` preferences
  for startup, successful reload, transformation, double-copy bypass, and
  plugin-attention notifications without hiding failures or tray status.
- **Packaging/Flatpak:** Add one offline Flatpak containing the desktop app and
  sandboxed CLI, GitHub Release bundles with checksums and provenance, and a
  signed shared update repository at `flatpak.jag-k.dev`. Adapt autostart and
  tray D-Bus registration to the Flatpak sandbox.
- **Packaging/Nix:** Add a project flake for Linux and macOS app/CLI builds,
  native macOS `.app` bundles, cross-platform CI, and prebuilt outputs in the
  public `jag-k` Cachix cache. Keep Intel macOS on the maintained
  `nixpkgs-26.05-darwin` branch.

### Changed

- **Documentation:** Organize installation by operating system, separate
  package-manager and direct-download options, distinguish GUI and CLI
  artifacts, and document Flatpak sandbox behavior, explicit CLI invocation,
  Nix application paths, and Cachix setup.

## [0.1.3] - 2026-07-31

### Added

- **Desktop/Runtime:** Add a clonable native wake handle, coalesced
  application-defined wake reasons, neutral native event reporting, and
  explicit runtime deadlines so the desktop can sleep until relevant work is
  ready without moving application policy into the host loop.
- **Tray/macOS:** Reconcile retained `NSMenuItem`, submenu, and SF Symbol
  objects in place when the semantic menu model changes.

### Changed

- **Release:** Use the version number by itself as the GitHub Release title.
- **Desktop/Runtime:** Replace the unconditional all-source 200 ms desktop tick
  with source-selective command, rule-result, configuration, and clipboard
  processing. Keep a deadline-based clipboard compatibility poll only where a
  backend or desktop session has no reliable native change notification.
- **Desktop/Runtime:** Derive wakeups and deadlines from tray and notification
  commands, completed rule jobs, filesystem events, remote-import refresh,
  clipboard contention retries, and native platform messages. This keeps the
  shared `AppCommand` ordering surface while avoiding unrelated clipboard and
  configuration probes.
- **Runtime/Imports:** Refresh URL imports with HTTP validators, preserve
  byte-identical cache files, and skip full config parsing and rule compilation
  when every remote source is unchanged.
- **Tray/macOS:** Preserve native menu objects across opens, update only changed
  fields and children, rebuild command tags atomically, and cache the bounded
  set of application-owned SF Symbols. Relative timestamps are still computed
  from a fresh semantic model whenever the menu opens.
- **Tray/Icons:** Describe generated tray pixels with an explicit format and
  stride, use a validated @2x grayscale-plus-alpha template payload on macOS,
  and retain RGBA payloads where Windows and Linux need color or theme
  variants.
- **Tray/Actions:** Add macOS SF Symbols for Undo and Edit rule and refine the
  Copy config path and Quit action icons.

### Fixed

- **Windows/Installer:** Statically link the C runtime into the desktop and
  CLI executables so a clean Windows installation can launch them without a
  separate Visual C++ Redistributable.
- **Windows/Desktop:** Embed the Clipboard Transformer product name in the GUI
  executable so Task Manager does not show the internal `ct-desktop` package
  name.
- **Desktop/Runtime:** Prevent a wakeup sent between the final application drain
  and the native wait from leaving commands, configuration events, or completed
  transformations pending indefinitely.
- **Windows/Runtime:** Observe clipboard changes through the message-only Win32
  listener, route native quit through the ordered application command channel,
  and fall back to compatibility polling instead of failing startup when the
  clipboard listener is unavailable.
- **macOS/Runtime:** Stop calling `NSApplication::updateWindows()`
  unconditionally and create tray UI only after the native application pump is
  initialized.
- **Desktop/Runtime:** Filter config-watch events before enqueueing or waking
  the host, so log, history, state, and PID writes cannot create a
  self-amplifying filesystem/logging loop. Keep dependency diagnostics at
  `Info` unless verbose tracing is explicitly added later.
- **macOS/Tray:** Use an explicit square, image-only status item and a stable
  application-specific autosave name so AppKit can persist its position and
  visibility independently.

## [0.1.2] - 2026-07-29

### Changed

- **Windows/Installer:** Make the local `just install-app-windows` helper use
  the same silent MSI flags and verbose installer logging as the verification
  script.

### Fixed

- **Windows/Runtime:** Avoid a heap-corruption crash while refreshing portable
  toast shortcut metadata by letting `PROPVARIANT` values clean themselves up
  exactly once.
- **Windows/Scoop:** Reuse and enrich an existing Start Menu shortcut for the
  current executable, including Scoop-created shortcuts, and remove the old
  app-owned fallback shortcut to avoid duplicate Start Menu entries.

## [0.1.1] - 2026-07-28

### Added

- **Windows/MSI:** Verify clean installation, upgrades from the public `0.1.0`
  MSI, the installed app and CLI, machine `PATH`, Start Menu and toast
  registration, and complete uninstall cleanup in Windows CI.
- **Release/WinGet:** Upload the fully rendered manifest set before submission
  so the exact package-catalog changes remain available for inspection.

### Changed

- **Runtime/HTTP:** Download URL imports and plugin modules with an in-process
  HTTP client, retaining hard timeouts and download-size limits without
  requiring an external `curl` executable.
- **Release:** Keep stable manual-download links, Homebrew metadata, and Scoop
  metadata unchanged during prereleases, then update them automatically for
  the next stable release.

### Fixed

- **Windows/Runtime:** Prevent background shell rules, process-tree cleanup,
  and other helper commands from opening console windows.
- **Windows/MSI:** Keep the optional CLI directory on machine `PATH` after
  silent installation and major upgrades by assigning its component directly
  to the WiX Environment feature.
- **Release/WinGet:** Preserve inline release notes, versioned license,
  documentation and icon URLs, and the icon SHA-256 when generating an updated
  catalog manifest.
- **Release/Changelog:** Generate concrete comparison links during
  `cargo release` instead of leaving unsupported template placeholders in the
  release commit.

### Documentation

- **Installation:** Add direct downloads for every published macOS, Windows,
  and Linux app and CLI format, clarify which distributions include the CLI,
  and document checksum and GitHub attestation verification.
- **Platforms:** Replace pre-release package guidance with the published
  Homebrew, Scoop, AUR, AppImage, DEB, RPM, Pacman, MSI, and portable
  installation paths while retaining explicit signing and Linux validation
  boundaries.
- **Release:** Expand maintainer runbooks for MSI upgrade verification,
  Windows signing, WinGet bootstrap and metadata review, and future Linux
  catalog expansion through AppImageHub, COPR, Launchpad, OBS, and Nixpkgs.

## [0.1.0] - 2026-07-28

### Added

- Native desktop hosts for macOS, Windows, and Linux alongside the standalone
  CLI.
- Rule imports, clipboard history and undo, notifications, and optional WASM
  plugins.
- Signed and notarized macOS packages, Windows portable executables and MSI,
  and Linux AppImage/DEB/Pacman/RPM packages, plus automated Homebrew, Scoop,
  AUR, and WinGet publishing.

<!-- next-url -->
[Unreleased]: https://github.com/jag-k/clipboard-transformer/compare/v0.1.6...HEAD
[0.1.6]: https://github.com/jag-k/clipboard-transformer/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/jag-k/clipboard-transformer/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/jag-k/clipboard-transformer/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/jag-k/clipboard-transformer/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/jag-k/clipboard-transformer/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/jag-k/clipboard-transformer/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/jag-k/clipboard-transformer/releases/tag/v0.1.0
[Keep a Changelog]: https://keepachangelog.com/en/1.1.0/
[Semantic Versioning]: https://semver.org/spec/v2.0.0.html
