# Config and Data Paths

Prefer `clipboard-transformer paths` when available.

The CLI is not required to initialize configuration. On first desktop launch,
the app creates `config.yaml` and the adjacent editor schema when neither YAML
nor TOML config exists. `clipboard-transformer config init` is the optional
terminal equivalent and never overwrites an existing config.

Non-empty XDG variables independently override platform defaults:

- `XDG_CONFIG_HOME` -> `<value>/clipboard-transformer/`
- `XDG_STATE_HOME` -> `<value>/clipboard-transformer/`
- `XDG_CACHE_HOME` -> `<value>/clipboard-transformer/`

Fallbacks:

| Platform | Config | State | Cache |
| --- | --- | --- | --- |
| macOS | `~/Library/Application Support/dev.jag-k.clipboard-transformer/config/` | `~/Library/Application Support/dev.jag-k.clipboard-transformer/state/` | `~/Library/Caches/dev.jag-k.clipboard-transformer/` |
| Linux | `~/.config/clipboard-transformer/` | `~/.local/state/clipboard-transformer/` | `~/.cache/clipboard-transformer/` |
| Windows | `%APPDATA%\jag-k\clipboard-transformer\config\` | `%LOCALAPPDATA%\jag-k\clipboard-transformer\data\state\` | `%LOCALAPPDATA%\jag-k\clipboard-transformer\cache\` |

Flatpak: config, state, and cache default to the sandbox's XDG directories
below `~/.var/app/dev.jagk.clipboard_transformer/`. After
`flatpak override --user --filesystem=xdg-config/clipboard-transformer:create`,
the host `${HOST_XDG_CONFIG_HOME:-~/.config}/clipboard-transformer/` is used
once it holds `config.yaml` or `config.toml`; until then the sandbox config
stays active. State and cache always stay in the sandbox.

Check `<config_dir>/config.yaml`, then `config.toml`. Derived locations:

- generated schema: `<config_dir>/clipboard-transformer.schema.json`;
- plugins: `<config_dir>/plugins/*.wasm`;
- remote import cache: `<state_dir>/url-imports/`;
- group state: `<state_dir>/groups.json`.
