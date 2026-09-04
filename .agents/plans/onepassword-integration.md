# 1Password secret integration

## Status

Accepted future design; implementation is tracked in [`TODO.md`](../../TODO.md).
This document describes the intended scope and boundaries before code is added.

## User-facing goal

Clipboard Transformer will optionally load credentials from 1Password without
requiring the `op` CLI:

- a whole `.env` value may be an `op://vault/item/field` secret reference;
- a whole string value in the authored YAML or TOML configuration may be the
  same reference, including plugin settings;
- the user may provide a 1Password Environment ID, whose variables become an
  input to the application's effective environment.

References and the Environment ID are safe configuration metadata. Resolved
values are in-memory secrets and must never be written to the generated schema,
state/cache files, logs, diagnostics, notifications, or configuration output.

The initial feature resolves only a complete string value beginning with
`op://`. It deliberately does not implement interpolation inside a larger
string. This keeps secret handling explicit and avoids unexpected changes to
regular expressions, shell commands, URLs, and configuration diagnostics.

## Configuration contract

The host-owned settings belong under `config.onepassword`:

```yaml
config:
  onepassword:
    # 1Password account name or UUID used by desktop-app authorization.
    account: example.1password.com
    # Optional Environment ID copied from 1Password Developer > Environments.
    environment: 01234567-89ab-cdef-0123-456789abcdef
```

Both values are literal metadata; they cannot themselves be secret references.
`onepassword` is absent by default, so existing installations retain their
current behavior and do not pay an authorization prompt or network cost.

## Extensible provider boundary

1Password is the first secret provider, not a special case embedded throughout
the config loader. The runtime owns a small internal `secrets` boundary with
these conceptual operations:

- resolve one complete, provider-native reference into a secret value;
- optionally load a named provider environment into key/value variables; and
- report an explicit, redacted error category when an operation is unsupported
  or fails.

Each provider declares whether it supports reference resolution and/or an
environment import. The host, rather than a provider, owns parsing, source
tracking, precedence, reload behaviour, redaction, and the rule/plugin-facing
effective environment. A provider receives only its literal configuration and
the already-classified reference or environment selector. It must not inspect
or rewrite arbitrary authored configuration.

`op://` remains 1Password's native reference syntax. Do not claim that it is a
cross-manager standard. A second provider may first use its native syntax; if
users need provider selection inside one value, introduce a documented neutral
envelope such as `secret://<provider>/<opaque-reference>` at that time. It must
preserve the opaque provider portion byte-for-byte and never guess a provider
from a URL-shaped string.

Provider credentials are bootstrap inputs and cannot be resolved through the
same provider. They must come from the original process environment, an
operating-system credential store, or an explicit future interactive flow. This
prevents circular configuration such as needing a Bitwarden token in order to
read the Bitwarden token.

Keep this boundary inside `crates/runtime` initially. Provider implementations
can use Cargo features and isolated build inputs, so adding one does not force
another provider's SDK or bridge into every build. Extracting a separate crate
needs evidence from at least two implementations; do not manufacture a public
plugin API before that need exists.

## Candidate providers after the first increment

The first delivery is 1Password plus the local `os` provider below. The
provider boundary is deliberately intended to accommodate later, separately
approved integrations:

- Bitwarden Secrets Manager;
- Infisical;
- HashiCorp Vault or OpenBao;
- AWS Secrets Manager; and
- Consul KV only for compatibility with an existing, suitably protected Consul
  deployment, not as a recommended new secrets-management system.

These are candidates, not a promise to ship every integration. Each needs its
own audit, authentication/bootstrap contract, reference syntax, environment
capabilities, error handling, release-size budget, and provider-contract tests.
Do not generalize 1Password desktop authorization, `op://`, or Environments to
any of them.

## Local operating-system secret provider

Add an `os` provider for application-owned local secrets. A complete
`secret://os/<name>` value resolves the named value from the host credential
store; it is not a request to search the user's general password-manager
records. A future explicit `secrets set <name>`, `secrets delete <name>`, and
metadata-only `secrets list` CLI surface owns mutation. It must never provide a
command that prints a stored value.

This provider reuses the credential-storage contract in
[`plugin-authentication.md`](plugin-authentication.md), rather than inventing
a second keychain wrapper or JSON file. Its preferred backends are macOS
Keychain, Windows Credential Manager, and Linux Secret Service. When none is
available, it uses the same versioned, permission-restricted, atomically
written state-file fallback and the same actionable warning/`doctor` status.

The shared store has namespaced identities. Plugin authentication retains its
existing `(plugin_id, connection_id)` namespace; local secret references use
an application namespace plus `<name>`. A plugin does not receive a generic
keychain read/write API and cannot resolve or enumerate `secret://os/` values.
The common storage implementation is reusable, but authorization boundaries
remain distinct.

## Loading and precedence

For a file-backed desktop or CLI configuration, load in this order:

1. Parse the authored config and imports without resolving secret values.
   This obtains literal `config.onepassword` metadata without exposing secrets
   to import/path processing.
2. If 1Password is configured, authenticate through the installed 1Password
   desktop app and fetch the configured Environment.
3. Parse the adjacent `.env`, resolving complete `op://` values.
4. Build the existing effective environment with the established precedence:
   original process environment, adjacent `.env`, 1Password Environment,
   then the Unix login-shell snapshot where applicable.
5. Resolve complete `op://` values in the already-parsed configuration, then
   initialize plugins and compile rules.

The inline CLI configuration path remains self-contained and does not gain
implicit filesystem access. It may resolve `op://` configuration values only
when explicit 1Password metadata is supplied through a future CLI option; the
initial file-backed feature must not silently invent that metadata source.

A config-file change or explicit desktop Reload re-fetches the Environment and
re-resolves references. The background watcher must not poll 1Password on a
timer merely to discover remote secret rotation. A later polling feature needs
its own interval, failure policy, and user-visible security review.

## Authentication and SDK boundary

Use the official 1Password Go SDK with its desktop-app authorization flow.
The 1Password app authorizes the calling process locally; no personal password,
session token, or `op` CLI invocation is handled by Clipboard Transformer.

Do not depend on `corteq-onepassword` or another unofficial Rust wrapper. The
reviewed candidate is unsuitable because it is AGPL-3.0-or-later, supports
only service-account tokens, lacks the Environment API and Windows support,
and downloads a native library during Cargo builds.

The first implementation targets local desktop-app authorization. Service
accounts are a separate product/security decision: their token provisioning,
least-privilege scope, headless use, and logging/redaction contract must be
designed before support is added.

## Static Go bridge

Keep the language boundary deliberately narrow. A small Go `main` package
wraps only:

- create/close a client using desktop authorization;
- resolve one or many `op://` references;
- retrieve variables for one Environment ID;
- release a bridge-owned response buffer.

Build it with `go build -buildmode=c-archive`. Cargo links the resulting static
archive into the final Clipboard Transformer executable, rather than loading a
sidecar process or shipping a bridge DLL/shared object next to it. This lets
the 1Password authorization prompt identify Clipboard Transformer itself.

The archive and its generated C header are build inputs, not downloaded at
Cargo build time. Release CI builds a pinned Go module for every supported
native target, then links it using the platform external linker. CGO and the
matching cross C toolchain are required for desktop-app authorization.

Go performs reachability-based removal within its own build. The final Rust
linker may omit unused archive members and strip symbols, but it cannot perform
cross-language LTO or inline Go implementation into Rust. Keep the bridge API
small and apply Go release flags such as `-trimpath` and `-ldflags=-s -w`.

## Security and failure behavior

- The bridge accepts and returns length-delimited UTF-8 bytes; no NUL-terminated
  secret protocol or shell command is used.
- Rust owns all logging. It logs operation class and redacted error category,
  never secret values, references, Environment values, or secret-derived
  fingerprints.
- Go buffers are freed immediately through the bridge API. Rust avoids cloning
  secrets and clears temporary mutable buffers where practical; ordinary Rust
  `String` values cannot promise complete zeroization, so their lifetime stays
  tightly bounded.
- A failed startup secret resolution fails configuration loading with a clear,
  redacted diagnostic. During desktop reload, failure keeps the last valid
  engine running, matching existing reload safety behavior.
- The feature does not alter the process environment. It updates only the
  existing in-memory effective-environment layer used by plugin expansion and
  child commands.

## Delivery gates

Before enabling the feature in a release:

1. Build a proof of concept for macOS arm64, macOS x86_64, Windows x86_64, and
   Linux x86_64; verify that each final executable has no application-owned
   bridge shared library beside it.
2. Verify 1Password prompts identify the packaged Clipboard Transformer binary
   and that lock/inactivity revocation behaves as documented.
3. Test success and redacted failures for a reference, Environment, unavailable
   1Password app, denied authorization, malformed reference, and a changed
   Environment ID.
4. Add source-tracking/config-loader coverage, desktop reload coverage, and
   mock bridge contract tests. Keep a separately documented manual live test.
5. Measure release artifact size and cold/warm startup RSS against a build
   without the feature. Establish a budget before widening the bridge API.
6. Update the generated config schema, `docs/configuration.md`, CLI help where
   applicable, and the Clipboard Transformer rule-maintainer skill in the same
   implementation change.
7. Before shipping a second provider, add provider-contract tests for the
   capability matrix, unsupported Environment imports, opaque-reference
   handling, bootstrap-cycle rejection, and redacted errors. Do not infer that
   one provider's authentication or URI form applies to another.
