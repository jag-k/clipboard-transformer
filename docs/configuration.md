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
| `import_refresh_interval` | `600` | Remote-import refresh interval. `0` uses cache only. |
| `apps` | `[]` | Global source-application filter. |
| `app_mode` | unset | `blacklist` or `whitelist`; required when `apps` is non-empty. |
| `editor` | unset | Command and arguments used by **Edit rule**. |
| `shell.enabled` | `false` | Authorize trusted native shell rule providers. |

## Rule fields

Every real rule requires a stable, non-empty `id`. Common optional fields:

- `name`: display label; falls back to `id`;
- `formats`: ordered input priority for text rules, or presence filter for a
  ruleset;
- `apps`: source application identifiers;
- `app_mode`: `blacklist` or `whitelist`, required with non-empty `apps`.

Portable format aliases include `text`, `url`, `html`, `rtf`, and `file`.
Exact native identifiers printed by
`clipboard-transformer clipboard inspect` are also accepted. An omitted or
empty `formats` list defaults to `[text]`.

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
