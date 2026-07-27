# CLI reference

The standalone `clipboard-transformer` binary shares configuration, rules,
plugins, and native clipboard readers with the desktop app. It does not own the
tray runtime and never silently becomes a daemon.

You do not need the CLI to use the desktop app. This reference is for terminal
users, scripts, diagnostics, and integrations.

| Command | Purpose |
| --- | --- |
| `paths` | Print resolved config, plugin, state, cache, and app paths. |
| `doctor` | Print platform capability and path diagnostics. |
| `config init [--config-file path]` | Create starter YAML and its schema without overwriting an existing config. |
| `config check [config source]` | Expand imports, discover plugins, compile rules, and report issues. |
| `config schema` | Print or write built-in and plugin-aware JSON Schema. |
| `rules list [config source] [--available-only]` | List known built-in, native shell, and plugin rule types. |
| `rules view effective [config source] [--format json\|yaml]` | Print normalized rules, warnings, and source locations. |
| `clipboard inspect` | Describe the persisted latest item or explicitly read the current clipboard. |
| `clipboard watch [--format text\|jsonl] [--transform[=both\|transformed-only]]` | Observe clipboard changes without writing to the clipboard. |
| `transform [config source]` | Transform the current clipboard once and write the result back. |
| `transform --preview [config source]` | Show the current clipboard transformation without writing it. `--dry-run` is an alias. |
| `transform - [config source] [--input-format value]` | Read UTF-8 stdin and write only the final text to stdout. |
| `plugin ...` | Install, inspect, diagnose, list, or reload plugins. |

Run `clipboard-transformer <command> --help` for complete option details.

## Transform and preview

Omitting the input source means the current clipboard:

```sh
clipboard-transformer transform --preview
clipboard-transformer transform
```

The explicit `-` selects the stdin/stdout pipeline:

```sh
printf '%s' 'https://example.com/?utm_source=test&id=42' |
  clipboard-transformer transform -
```

Stdin represents plain text by default. Use `--input-format url` (or another
configured clipboard format) when testing a rule restricted to that format.

Every rule-loading command uses `--config` for inline YAML/TOML and
`--config-file` for a file. Omitting both selects the system config. Inline
config discovers no plugins unless `--plugin-dir` is also given and does not
expand imports.

## Effective rules

```sh
clipboard-transformer rules view effective
```

For file and system configs, the output is a diagnostic, loader-filtered
representation after import expansion. Inline configs are self-contained and
do not expand imports. The output is not a lossless authored YAML/TOML syntax
tree and not the private compiled execution plan.

## Available rule types

```sh
clipboard-transformer rules list
clipboard-transformer rules list --available-only
clipboard-transformer rules list --format json
```

The default list includes every known built-in and native rule type plus rule
descriptors from readable plugin manifests. Disabled shell rules and plugin
types unavailable under the selected configuration remain visible with their
status. `--available-only` filters them out.
