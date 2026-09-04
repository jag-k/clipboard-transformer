# TODO

Active project backlog. Completed work belongs in commits, pull requests, and
release notes rather than this file.

## Release readiness

- [ ] Style the release DMG with a deterministic Finder-free build or a
  prebuilt `.DS_Store`; keep signing, notarization, and stapling unchanged.
- [ ] Add Authenticode signing for the Windows MSI and portable executables.
  Apply to SignPath Foundation first; use Azure Artifact Signing when the
  developer or organization is region-eligible, otherwise use an OV
  certificate backed by a cloud HSM. Do not pay an EV premium solely for
  SmartScreen. Timestamp every signature and verify signed artifacts in CI.

## Runtime and tooling

- [ ] Add an optional, provider-based secret-resolution boundary. The first
  increment is complete `op://` resolution in `.env` and authored configuration,
  configured 1Password Environments, and application-local `secret://os/`
  references backed by the native credential store. The accepted design leaves
  room for later Bitwarden Secrets Manager, Infisical, Vault/OpenBao, Consul KV,
  and AWS Secrets Manager providers; use the
  [1Password integration plan](.agents/plans/onepassword-integration.md). Do
  not invoke the `op` CLI or adopt an unofficial Rust SDK.
- [ ] Add a configuration option to disable automatic config hot reload,
  including filesystem watches and periodic import polling, while keeping
  explicit manual reload available.
- [ ] Design explicit authored, validated, and compiled rule representations
  before exposing a stable inspection or editor API. Preserve import source
  locations and do not retain duplicate trees in the desktop process without a
  measured need.

## Future work

- [ ] Build a project website with clearer user-facing documentation, examples,
  downloads, and links to the full configuration and plugin references. Keep
  the repository README short and GUI-first; the website must not become a
  second, drifting source of truth for configuration semantics.
- [ ] Add host-managed plugin authentication in a future Plugin API revision.
  The accepted boundary is summarized in
  `.agents/plans/plugin-authentication.md`.
- [ ] Prototype a browser/WebExtension host for the portable core. Keep
  clipboard events, persistence, networking, and plugin execution honest about
  browser capability boundaries.
