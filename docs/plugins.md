# WASM plugins

Plugins add sandboxed namespaced rule types. A plugin is one core WebAssembly
module placed in the directory printed by:

```sh
clipboard-transformer plugin paths
```

The desktop app discovers `*.wasm` files at startup and on hot reload. It reads
the embedded `clipboard-transformer/manifest` custom section without executing
plugin code.

## Install and inspect

Copy a trusted module into `<config_dir>/plugins/`, or install one from an
HTTP(S) URL:

```sh
clipboard-transformer plugin install https://example.com/plugin.wasm
clipboard-transformer plugin list
clipboard-transformer plugin inspect dev.example.links
clipboard-transformer plugin doctor dev.example.links
clipboard-transformer plugin example dev.example.links
```

## Configure

Plugin rules use `<plugin-id>/<rule-type>`:

```yaml
plugins:
  dev.example.links:
    permissions:
      http: [example.com]
      env_expansion: true
    settings:
      token: ${EXAMPLE_TOKEN}

rules:
  - type: dev.example.links/human-readable-link
    id: example-links
    formats: [url, text]
```

`settings` is opaque plugin-owned JSON. `permissions` is host-owned; effective
capabilities are always the intersection of manifest requests and user grants.
Network access and environment expansion are denied by default.

Plugin failures, timeouts, and invalid rule settings become structured issues
and warnings rather than application startup failures.

## Author a plugin

The authoring artifacts and generation workflow are in [`plugins/`](../plugins/README.md):

- [Plugin API v1 contract](../plugins/API.md);
- [generated XTP schema](../plugins/plugin-api-v1.xtp.yaml);
- [generated manifest schema](../plugins/manifest.schema.json);
- [Rust example plugin](../plugins/gitlab-link/README.md).

Plugin API v1 is a narrow selected-text transform contract. Arbitrary
multi-format clipboard reads and writes are not exposed to plugins.
