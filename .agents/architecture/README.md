# Architecture

This is the durable architecture overview. Source code and public contracts
remain authoritative; completed migration diaries and exploratory research are
intentionally not kept in the repository.

## Product surfaces

Clipboard Transformer exposes two related but distinct products:

- the desktop application continuously observes and may rewrite the system
  clipboard; it owns native UI, notifications, history, reload, and autostart;
- the CLI performs explicit inspection, validation, testing, clipboard reads,
  and stdin/stdout transforms without becoming a background service.

They share configuration loading, the rule engine, plugin discovery, and the
portable clipboard model.

## Package boundaries

- `crates/core`: portable transformation engine and built-in rules.
- `crates/config`: portable YAML/TOML document types and string parsing.
- `crates/clipboard`: clipboard item model, codecs, and backend contract.
- `crates/tray` and `crates/notifications`: platform-neutral UI contracts.
- `crates/plugin-api`: runtime-neutral guest protocol.
- `crates/runtime`: filesystem config loading, imports, plugins, persistence,
  application behavior, and native platform composition.
- `apps/desktop` and `apps/cli`: thin product entry points.

Native callbacks feed one `AppCommand` channel drained serially by `Agent`.
This preserves ordering between clipboard changes, tray actions, and
notification actions.

## Configuration boundary

The desktop app creates a starter YAML config on first launch. YAML and TOML
then pass through the same loader. Local and remote imports contribute rules
only; URL imports are cached in state. The generated schema sits beside the
active config but is not embedded in release packages or committed.

## Plugins

The public protocol is independent of Extism. Only the runtime adapter knows
the concrete WASM host. Plugin capabilities are the intersection of manifest
requests and user grants, and plugin failures remain isolated from the rest of
the application.

The current author contract is [`plugins/API.md`](../../plugins/API.md).
Potential host-managed authentication is summarized in
[`../plans/plugin-authentication.md`](../plans/plugin-authentication.md).
The accepted future direction for rule-group metadata and shared local group
state is recorded in [`../plans/rule-groups.md`](../plans/rule-groups.md).

## Platforms

macOS, Windows, and Linux are supported desktop targets. Each owns its native
clipboard backend, tray, notifications, host loop, and autostart integration.
Shared crates define semantics, not a lowest-common-denominator UI toolkit.

Current user-visible platform behavior belongs in
[`docs/platforms.md`](../../docs/platforms.md) and
[`docs/linux.md`](../../docs/linux.md). Remaining work belongs in
[`TODO.md`](../../TODO.md).
