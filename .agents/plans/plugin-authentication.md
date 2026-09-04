# Host-managed plugin authentication

Status: accepted direction for a future Plugin API revision; not implemented
and not part of Plugin API v1.

## Boundary

The host owns user interaction, credential storage, refresh scheduling, and
connection lifecycle. A plugin owns provider-specific fields and how a
credential is applied to its allowed HTTP requests.

Granting a plugin a credential means trusting that plugin with the credential.
WASM isolation and capability grants constrain the module but cannot hide a
secret from code that must use it.

## Connection model

A connection is identified by `(plugin_id, connection_id)`. The plugin supplies
a stable non-secret `connection_id`; labels, provider names, and settings order
are presentation details rather than identity. One plugin may expose several
providers or accounts.

The host tracks states such as missing, authorizing, available, refreshing,
reauthorization-required, failed, and disconnected. Missing authentication
degrades only the affected plugin functionality.

## Proposed protocol shape

A future manifest requests a generic authentication capability. Runtime
initialization reports required connection IDs and reasons. Optional plugin
exports describe a flow, consume its result, and refresh credentials; a host
import returns the current credential immediately before use.

The flow description must be declarative. Plugins never command the host to
open arbitrary URLs or UI. Initial supported flow families may include:

- OAuth authorization code with PKCE;
- device authorization;
- user-entered secrets;
- plugin-defined browser or manual exchanges expressed through bounded,
  host-rendered steps.

Credential-bearing values require redacted diagnostics and must never enter
logs, generic error serialization, config files, or the Extism variable store.
Storage should use the native secret store when available, with an explicit
user-visible fallback policy where it is not.

The same host-owned credential-storage implementation may serve the runtime's
application-local `secret://os/<name>` provider, as described in
[`onepassword-integration.md`](onepassword-integration.md). This does not grant
plugins a generic Keychain API: their records remain scoped to
`(plugin_id, connection_id)`, distinct from application-local secret records.

## Lifecycle requirements

- Discover requirements during initialization.
- Complete authorization through host-owned UI.
- Persist credentials atomically before reinitializing the plugin.
- Refresh before expiry with bounded retry and backoff.
- Serialize credential mutation per connection.
- Revoke or disconnect without affecting unrelated connections.
- Detect settings changes with a non-secret configuration fingerprint.
- Never make authentication failure a whole-application startup failure.

Before implementation, turn this outline into versioned protocol types and
tests in `crates/plugin-api`, then update
[`plugins/API.md`](../../plugins/API.md).
