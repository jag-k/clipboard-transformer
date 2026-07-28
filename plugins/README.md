# Clipboard Transformer plugins

This directory contains the generated Plugin API schema, plugin-generation
instructions, and the canonical Rust example.

- [`API.md`](API.md) is the complete human-readable plugin contract.
- [`plugin-api-v1.xtp.yaml`](plugin-api-v1.xtp.yaml) is the generated wire
  schema used by XTP.
- [`manifest.schema.json`](manifest.schema.json) is the generated JSON Schema
  for complete embedded manifests and reviewable generated manifest files.
- [`gitlab-link/`](gitlab-link/README.md) is the canonical Rust example.

## Regenerate plugin schemas

All checked-in schemas are generated from the real Rust types in
`crates/plugin-api/src/lib.rs`:

```sh
just gen-schemas
```

Do not edit either file by hand. `cargo test` compares them with fresh output,
so protocol changes cannot silently leave them stale. Generation lives in a
separate Cargo binary and is not linked into the application runtime.

`just gen-schemas` covers only repository-owned plugin authoring artifacts.
The effective Clipboard Transformer config schema remains runtime-generated
next to the user's config and is not checked into this directory.

## Generate a plugin project

Install the XTP CLI, then generate a project from the published schema:

```sh
curl https://static.dylibso.com/cli/install.sh -s | bash

xtp plugin init \
  --schema-file https://raw.githubusercontent.com/jag-k/clipboard-transformer/main/plugins/plugin-api-v1.xtp.yaml \
  --path /tmp/my-clipboard-plugin \
  --name my-clipboard-plugin \
  --template Rust \
  --feature none \
  --yes
```

XTP accepts either a URL or a local path for `--schema-file`. Use
`plugins/plugin-api-v1.xtp.yaml` while developing unpublished API changes.
Replace `Rust` with another supported template when needed. XTP generates the
Extism exports and typed JSON payload layer, but not Clipboard Transformer's
embedded manifest. Add a `clipboard-transformer/manifest` custom section after
generation; [`gitlab-link/`](gitlab-link/README.md) shows one way to assemble
it from an authored `manifest.base.json` and Rust declarations at build time.

Read [`API.md`](API.md) for the manifest fields, export payloads, lifecycle,
capabilities, limits, and compatibility rules.

Generated bindings such as `src/pdk.rs` should be checked in. Regenerate them
when Plugin API changes, not during every normal plugin build.

The XTP schema declares both plugin exports and host-function imports. The
generated `pdk.rs` contains their ABI declarations and typed wrappers; do not
add those declarations by hand. Plugin code implements the generated exports
and calls generated imports. Clipboard Transformer still has to implement and
register each import on the host side because that code cannot live in the
guest WASM.

### Rust manifest assembly

The canonical example keeps manifest generation and rule behavior separate:

- `manifest.base.json` contains human-authored plugin identity, capabilities,
  and instructions;
- `build.rs` owns rule metadata, examples, schemas, and manifest assembly, then
  writes the complete, reviewable `manifest.json` only when it changes;
- `src/rules.rs` contains the actual compile, match, transform, and online
  lookup behavior for the plugin's rules;
- the same build step emits only the short rule type constants into `OUT_DIR`
  so the build metadata and rule implementation use the same identifiers.

`build.rs` and its metadata are not linked into the guest WASM. The manifest
metadata itself is present exactly once in the required custom section. A
plugin that does not need generated metadata may instead author one complete
`manifest.json` and embed it directly.

## Rust target: keep `wasm32-wasip1`

The official XTP Rust template defaults to `wasm32-wasip1`, and Clipboard
Transformer keeps that default:

```toml
# .cargo/config.toml
[build]
target = "wasm32-wasip1"
```

No target edits are needed after `xtp plugin init`. The host enables the basic
WASI context used by the template, including clocks and randomness, but does
not preopen filesystem directories. Plugin HTTP access and instance variables
remain Extism host capabilities controlled by the Clipboard Transformer
permission model.

Use another target only when the plugin has a concrete reason to diverge from
the template.

## Canonical Rust example

[`gitlab-link/`](gitlab-link/README.md) is the only canonical example:

- `src/pdk.rs` comes from the XTP Rust template;
- `src/lib.rs` implements the generated stubs;
- the manifest custom section is layered on top;
- `wasm32-wasip1` stays aligned with the template default;
- `just build-example-plugin` builds it and `just test-plugins` exercises it
  through the real host runtime.
