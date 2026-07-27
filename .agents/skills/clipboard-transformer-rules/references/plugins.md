# Plugin Rules

Plugin types use `<plugin-id>/<rule-type>`. Shared fields retain their meaning;
all other keys belong to that plugin.

Inspect instead of guessing:

```sh
clipboard-transformer rules list
clipboard-transformer rules list --available-only
clipboard-transformer plugin list
clipboard-transformer plugin inspect '<plugin-id>'
clipboard-transformer plugin doctor '<plugin-id>'
clipboard-transformer plugin example '<plugin-id>'
clipboard-transformer plugin paths
clipboard-transformer plugin install 'https://example.com/plugin.wasm'
clipboard-transformer plugin reload
```

Do not edit `plugins.<id>.permissions` merely to make a rule validate.

`rules list` reads rule descriptors from plugin manifests. Its default output
keeps unavailable or unconfigured plugin types visible with their status;
`--available-only` filters them using the selected config and plugin state.

`plugin install` accepts HTTPS and validates the embedded manifest before
moving the module into the plugin directory. `plugin reload` asks a running
desktop instance to reload config and plugins.
