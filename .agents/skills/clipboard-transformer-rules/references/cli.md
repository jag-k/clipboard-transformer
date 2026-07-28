# Rule-focused CLI

The desktop application does not require the CLI. Every rule-loading command
uses the same config-source contract:

- `--config-file <path>` loads a YAML or TOML file;
- `--config '<yaml-or-toml>'` loads a self-contained inline document;
- omitting both loads the system config;
- the two options conflict.

| Command | Config argument | Purpose |
| --- | --- | --- |
| `paths` | system config | Print resolved config, plugin, state, cache, and app paths. |
| `config init --config-file <path>` | YAML output path | Create starter YAML if absent and write its schema. |
| `config check [config source]` | file, inline, or system | Load, expand, compile, and report config issues. |
| `config schema --output <path>` | plugin-aware by default | Write JSON Schema; `--no-plugins` excludes plugin variants. |
| `rules list [config source]` | file, inline, or system | List known rule types and their availability. |
| `rules view effective [config source]` | file, inline, or system | Print normalized diagnostics as JSON or YAML. |
| `transform [config source]` | file, inline, or system | Transform the current clipboard once. |
| `transform --preview [config source]` | file, inline, or system | Show the current clipboard result without writing it. |
| `transform - --config-file <path>` | file path | Transform UTF-8 stdin to stdout. |
| `transform - --config '<yaml-or-toml>'` | inline document | Transform stdin with a self-contained config. |
| `clipboard watch --config-file <path>` | file path | Observe clipboard changes without writing them back. |
| `clipboard watch --config '<yaml-or-toml>'` | inline document | Observe using a self-contained config. |
| `clipboard inspect` | system config/state | Describe the persisted latest item or explicitly read the clipboard. |
| `doctor` | system paths | Print platform capabilities and path diagnostics. |

`config check`, `rules list`, `rules view effective`, `transform`, and
`clipboard watch` also accept `--plugin-dir <path>` and `--state-dir <path>`.
Inline config discovers no plugins unless `--plugin-dir` is provided, does not
expand imports, and conflicts with `--state-dir`.

`clipboard watch --transform` emits original and final transformed items;
`--transform=transformed-only` omits non-matches. Output format is
`--format text` or `--format jsonl`.

## Rule discovery and views

`rules list` includes every known core/native type and every rule descriptor
from readable plugin manifests, even when shell execution is disabled or a
plugin is not currently available. Use `--available-only` to filter by the
selected config and plugin state. Text output is the default;
`--format json` emits a versioned catalog.

`rules view effective` emits JSON by default; `--format yaml` serializes the
same versioned representation as YAML. The only implemented view is
`effective`. Future `authored` and `compiled` views require separate contracts
and must not be inferred from the effective representation.

## Validation

Validate and inspect a file:

```sh
clipboard-transformer config check --config-file '<path>'
clipboard-transformer rules view effective --config-file '<path>'
printf '%s' '<sample>' |
  clipboard-transformer transform - --config-file '<path>' --input-format url
```

In a repository checkout:

```sh
target/release/clipboard-transformer config check --config-file '<path>'
printf '%s' '<sample>' |
  cargo run --quiet --locked -- transform - --config-file '<path>' --input-format url
```

For file and system configs, validation expands imports. Inline configs are
self-contained and never expand imports. Validation discovers the selected
plugins, compiles built-ins, and reports cycles, duplicate ids, unavailable
types, malformed rules, unauthorized shell rules, and empty whitelists. The
effective view is normalized and loader-filtered; it is not lossless authored
YAML/TOML or a compiled-plan API.

Without an executable, review structure, schema, ids, filters, Rust-regex
compatibility, URL operations, imports, shell authorization, and examples.
Report that as structural review, not full validation.
