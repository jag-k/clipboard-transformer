# TODO

Active project backlog. Completed work belongs in commits, pull requests, and
release notes rather than this file.

## Release readiness

- [ ] Validate installed Linux packages on Xubuntu/X11, GNOME with XWayland,
  data-control Wayland/SteamOS, a session without a StatusNotifierHost, and an
  intentionally unsupported GNOME Wayland session. Cover tray actions,
  notifications, D-Bus activation, autostart, and uninstall.
- [ ] Validate the signed and notarized macOS artifacts, Homebrew cask, and
  Gatekeeper behavior during the first public release. Follow
  `.agents/runbooks/release/`.
- [ ] Style the release DMG with a deterministic Finder-free build or a
  prebuilt `.DS_Store`; keep signing, notarization, and stapling unchanged.
- [ ] Verify on Windows that Start Menu registration can skip rewriting an
  already-correct shortcut while preserving toast activation.
- [ ] Add Authenticode signing for the Windows MSI and portable executables.
  Prefer Azure Artifact Signing for public distribution, timestamp every
  signature, and verify the signed artifacts in CI before publication.

## Runtime and tooling

- [ ] Audit CI and `Justfile` together. Prefer shared `just` recipes where CI
  currently duplicates Cargo invocations with misleading feature flags.
- [ ] Diagnose the intermittent
  `unix_instance::tests::exclusive_lock_is_released_when_file_closes` failure
  that appears only during some fully parallel test runs.
- [ ] Design explicit authored, validated, and compiled rule representations
  before exposing a stable inspection or editor API. Preserve import source
  locations and do not retain duplicate trees in the desktop process without a
  measured need.

## Future work

- [ ] Add host-managed plugin authentication in a future Plugin API revision.
  The accepted boundary is summarized in
  `.agents/plans/plugin-authentication.md`.
- [ ] Prototype a browser/WebExtension host for the portable core. Keep
  clipboard events, persistence, networking, and plugin execution honest about
  browser capability boundaries.
- [ ] Consider extracting native tray, clipboard, notifications, and host-loop
  crates only after all three current platforms have stronger real-session
  validation and a concrete independent consumer exists.
