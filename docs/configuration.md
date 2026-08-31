# Configuration

Clipboard Transformer uses a small YAML file for everyday customization and
also accepts manually authored TOML. On first desktop launch, the app creates
the default `config.yaml` and editor schema if neither format exists. No
terminal command is required.

Use **Open config file** or **Show config in file manager** from the tray to
find the generated file. If you already work in a terminal,
`clipboard-transformer paths` prints all effective directories and
`clipboard-transformer config init` can create the starter files before the first
desktop launch. `init` never overwrites an existing config.

## Minimal config

```yaml
# $schema: ./clipboard-transformer.schema.json

config:
  recent_items_count: 5
  double_copy_window: 10

rules:
  - type: regexp
    id: example
    from: '^old$'
    to: 'new'
```

The app refreshes `clipboard-transformer.schema.json` beside the active config
after successful startup. Use it for completion and stricter typo detection.
Runtime parsing is intentionally permissive: invalid individual rules are
reported and skipped while valid rules continue.

## Paths

Non-empty XDG variables override their corresponding directory on every
platform:

| Variable | Directory |
| --- | --- |
| `XDG_CONFIG_HOME` | `<value>/clipboard-transformer/` |
| `XDG_STATE_HOME` | `<value>/clipboard-transformer/` |
| `XDG_CACHE_HOME` | `<value>/clipboard-transformer/` |

Without XDG overrides, native application directories are used. The main files
are:

| Platform | Config directory | State directory | Cache directory |
| --- | --- | --- | --- |
| macOS | `~/Library/Application Support/dev.jag-k.clipboard-transformer/config/` | `~/Library/Application Support/dev.jag-k.clipboard-transformer/state/` | `~/Library/Caches/dev.jag-k.clipboard-transformer/` |
| Linux | `~/.config/clipboard-transformer/` | `~/.local/state/clipboard-transformer/` | `~/.cache/clipboard-transformer/` |
| Windows | `%APPDATA%\jag-k\clipboard-transformer\config\` | `%LOCALAPPDATA%\jag-k\clipboard-transformer\data\state\` | `%LOCALAPPDATA%\jag-k\clipboard-transformer\cache\` |

The active file set is:

| Path | Purpose |
| --- | --- |
| `<config_dir>/config.yaml` | Default YAML config. |
| `<config_dir>/config.toml` | TOML alternative when YAML is absent. |
| `<active_config_dir>/.env` | Optional environment values for the active config. |
| `<config_dir>/clipboard-transformer.schema.json` | Generated editor schema. |
| `<config_dir>/plugins/*.wasm` | Discovered plugins. |
| `<state_dir>/url-imports/` | Remote import cache. |
| `<state_dir>/groups.json` | Group enablement state. |
| `<state_dir>/clipboard-transformer.log` | Runtime log. |
| `<state_dir>/history.cbor` | Recent transformation history. |

Prefer `clipboard-transformer paths` over hard-coding platform locations.

## Application options

| Option | Default | Meaning |
| --- | ---: | --- |
| `double_copy_window` | `10` | Seconds in which copying the original again bypasses a rewrite. |
| `recent_items_count` | `5` | Transformations retained in the tray. `0` disables history. |
| `max_item_bytes` | `104857600` | Maximum clipboard item size processed. |
| `max_history_bytes` | `536870912` | Maximum retained before/after history bytes. |
| `persist_last_clipboard` | `false` | Persist the complete latest external item for `inspect`. |
| `disable_for` | `600` | Seconds used by a notification's disable action. |
| `notifications.startup` | `true` | Notify after the desktop app starts successfully. |
| `notifications.reload_success` | `true` | Notify after a changed config is applied successfully. |
| `notifications.transform` | `true` | Notify after clipboard content is transformed. |
| `notifications.double_copy_ignored` | `true` | Notify when a double copy bypasses rules. |
| `notifications.plugin_attention` | `true` | Notify when plugins require attention. |
| `import_refresh_interval` | `600` | Remote-import refresh interval. `0` uses cache only. |
| `apps` | `[]` | Global source-application filter. |
| `app_mode` | unset | `blacklist` or `whitelist`; required when `apps` is non-empty. |
| `editor` | unset | Command and arguments used by **Edit rule**. |
| `shell.enabled` | `false` | Authorize trusted native shell rule providers. |

Notification preferences suppress only their named desktop notifications.
Disabling `transform` does not disable transformations or tray history, but its
notification actions such as **Undo**, **Edit rule**, and **Disable rule** are
not shown. Disabling `plugin_attention` does not hide plugin status from the
tray. Startup, runtime, and reload failure notifications remain enabled.

## Rule fields

Every real rule requires a stable, non-empty `id`. Common optional fields:

- `name`: display label; falls back to `id`;
- `groups`: list of group ids the rule belongs to;
- `formats`: ordered input priority for text rules, or presence filter for a
  ruleset;
- `apps`: source application identifiers;
- `app_mode`: `blacklist` or `whitelist`, required with non-empty `apps`.

Portable format aliases include `text`, `url`, `html`, `rtf`, and `file`.
Exact native identifiers printed by
`clipboard-transformer clipboard inspect` are also accepted. An omitted or
empty `formats` list defaults to `[text]`.

## Groups

Groups are a compact way to enable and disable related rules together. They
live in a flat global namespace; published packages should namespace their ids,
for example `@url-cleaner/privacy`.

```yaml
# $schema: ./clipboard-transformer.schema.json

groups:
  privacy:
    name: Privacy
    description: Removes tracking parameters and advertising identifiers
    status: visible

rules:
  - type: url-cleanup
    id: remove-tracking
    groups: [privacy]
    remove_query_prefixes: [utm_]
```

Rules, rulesets, and import edges may all carry `groups`. Membership is
additive: a ruleset's groups are inherited by its children, and import edge
groups are added to every rule imported through that edge. Set
`ignore_imported_groups: [id, ...]` or `ignore_imported_groups: true` on an
import to strip groups from the imported document before the edge groups are
applied.

A group descriptor controls presentation and default mutability:

- `status: visible` — functional and mutable from the tray or CLI;
- `status: hidden` — functional and mutable from the CLI, but not shown in the
  tray;
- `status: ignore` — removed from evaluation and cannot be enabled.

Undeclared groups used by rules are active by default and use the group id as
their label. They are functional but not shown in the tray.

Group descriptors can also be imported from other YAML/TOML files using
top-level `group_imports`. Imported descriptors default to `status: hidden`
unless the import edge sets a different status. Root descriptors always win;
between imports, a later entry overrides an earlier entry. Conflicting imported
descriptors produce a source-aware validation warning, and descriptor files are
watched like rule imports.

```yaml
group_imports:
  - source: shared-groups.yaml
    status: hidden
```

Group enablement state is stored in `<state_dir>/groups.json` as local,
versioned JSON. Toggle it with the CLI or the desktop tray:

```sh
clipboard-transformer groups list
clipboard-transformer groups enable privacy
clipboard-transformer groups disable privacy
```

Use `--group-state <path>` to select a state file and `--ignore-group-state` to
disable group state for one command. CLI and desktop updates use a shared
atomic read-modify-write lock, so concurrent toggles preserve unrelated group
changes.

Malformed state is never used for rule evaluation. The CLI reports the concrete
file and suggests repair, deletion, or `--ignore-group-state` for read-only
commands. An explicit `groups enable` or `groups disable`, or a tray toggle,
treats the state file as app-owned and rewrites it from the last in-memory
snapshot (or a fresh empty state if no snapshot has been loaded), then logs a
warning. The desktop keeps the last valid state in memory; if startup has no
valid state, grouped rules remain disabled until the file is repaired or an
explicit toggle rewrites it. State-file changes are watched separately from
configuration, so repairs and CLI toggles still apply while the authored config
is temporarily invalid.

The tray displays at most 64 visible group switches. Additional switches keep
their runtime semantics and are reported by a diagnostic row that opens the
config; they are never silently promoted, dropped from evaluation, or treated
as disabled.

## Environment

An optional `.env` beside the active config is loaded on every platform and
hot-reloaded by the desktop app. Existing process values win over `.env`.
Unix GUI launches first sample the default shell as a non-interactive login
shell, then overlay `.env`, then the original GUI environment.

Use `.env` for plugin settings such as `${GITLAB_TOKEN}`. Do not commit secrets.

## Validate changes

```sh
clipboard-transformer config check --config-file /path/to/config.yaml
printf '%s' 'old' |
  clipboard-transformer transform - --config-file /path/to/config.yaml
```

See [rules](rules.md), [imports](imports.md), and [plugins](plugins.md).
