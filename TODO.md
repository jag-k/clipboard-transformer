# TODO

Active project backlog. Completed work belongs in commits, pull requests, and
release notes rather than this file.

## Release readiness

- [ ] Bootstrap `JagK.ClipboardTransformer` in `microsoft/winget-pkgs` with the
  first MSI containing the corrected CLI PATH feature ownership, complete its
  install/upgrade/uninstall review, add
  `WINGET_CREATE_GITHUB_TOKEN`, and enable automated stable updates.
- [ ] Add the published AppImage to AppImageHub for discovery without creating
  another package format.
- [ ] Publish an RPM source recipe in COPR, then add a signed Debian source
  package to Launchpad PPA, and finally reuse both recipes in OBS. Vendor Cargo
  dependencies and verify the required Rust toolchain in each remote builder.
- [ ] Validate installed Linux packages on Xubuntu/X11, GNOME with XWayland,
  data-control Wayland/SteamOS, a session without a StatusNotifierHost, and an
  intentionally unsupported GNOME Wayland session. Cover tray actions,
  notifications, D-Bus activation, autostart, and uninstall.
- [ ] Validate the signed and notarized macOS artifacts, Homebrew cask, and
  Gatekeeper behavior from the published release. Follow
  `.agents/runbooks/release/`.
- [ ] Style the release DMG with a deterministic Finder-free build or a
  prebuilt `.DS_Store`; keep signing, notarization, and stapling unchanged.
- [ ] Verify on Windows that Start Menu registration can skip rewriting an
  already-correct shortcut while preserving toast activation.
- [ ] Add Authenticode signing for the Windows MSI and portable executables.
  Apply to SignPath Foundation first; use Azure Artifact Signing when the
  developer or organization is region-eligible, otherwise use an OV
  certificate backed by a cloud HSM. Do not pay an EV premium solely for
  SmartScreen. Timestamp every signature and verify signed artifacts in CI.

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

- [ ] Consider an upstream Nixpkgs package after a project-owned flake has been
  tested in a real NixOS desktop session.
- [ ] Add host-managed plugin authentication in a future Plugin API revision.
  The accepted boundary is summarized in
  `.agents/plans/plugin-authentication.md`.
- [ ] Prototype a browser/WebExtension host for the portable core. Keep
  clipboard events, persistence, networking, and plugin execution honest about
  browser capability boundaries.
- [ ] Consider extracting native tray, clipboard, notifications, and host-loop
  crates only after all three current platforms have stronger real-session
  validation and a concrete independent consumer exists.
