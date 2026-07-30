# Changelog

All notable changes to Clipboard Transformer are documented in this file.

The format is based on [Keep a Changelog], and this project follows
[Semantic Versioning].

<!-- next-header -->
## [Unreleased] - ReleaseDate

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
[Unreleased]: https://github.com/jag-k/clipboard-transformer/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/jag-k/clipboard-transformer/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/jag-k/clipboard-transformer/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/jag-k/clipboard-transformer/releases/tag/v0.1.0
[Keep a Changelog]: https://keepachangelog.com/en/1.1.0/
[Semantic Versioning]: https://semver.org/spec/v2.0.0.html
