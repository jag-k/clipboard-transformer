# AGENTS.md

Guidance for people and agents working in this repository.

## Scope and sources of truth

- Clipboard Transformer has two user surfaces: a native desktop application
  and a standalone CLI. They share rules and configuration but serve different
  workflows. Do not present the CLI as required desktop setup.
- Keep `README.md` short and GUI-first. `docs/` is user-facing documentation;
  `.agents/` contains only durable maintainer records, release runbooks, and
  portable skills.
- Verify behavior claims against `crates/runtime/src/config`,
  `crates/runtime/src/app`, `apps/desktop`, and `apps/cli/src/cli.rs`.
- Prefer small direct changes and preserve unrelated worktree edits.

## Configuration

- YAML is the default format; manually authored TOML is supported.
- First desktop launch creates `config.yaml` and its adjacent JSON Schema when
  neither YAML nor TOML exists. `clipboard-transformer config init` is optional
  and never overwrites an existing config.
- Path resolution is XDG-first. Without XDG overrides, use the directories
  resolved by `directories::ProjectDirs`. URL imports are cached under
  `<state_dir>/url-imports/`.
- `import` is a rule entry. It accepts relative and absolute paths, `file:`,
  `http:`, and `https:` URLs. Relative imports resolve from the importing file;
  YAML and TOML may import each other; only `rules` are imported.
- Runtime startup writes the effective schema beside the active config. Keep
  `crates/runtime/src/config/schema.rs` aligned with parsing behavior and never
  commit a generated config schema.
- When rule types, shared fields, rule semantics, imports, or CLI commands and
  options change, update
  `.agents/skills/clipboard-transformer-rules/SKILL.md` and its references in
  the same change. Validate the skill before handing off; do not leave its
  examples or command syntax behind the application.
- An optional `.env` beside the active config is loaded on every platform. The
  desktop watcher reloads it; an explicit desktop reload also resamples the
  Unix login-shell environment.

## Desktop and CLI boundaries

- The desktop application owns clipboard writes, tray UI, notifications,
  history, hot reload, single-instance behavior, and autostart.
- The CLI owns explicit commands and Unix-style pipelines. It must not silently
  start a daemon or provide a second autostart path.
- Native callbacks publish `AppCommand` into the one channel drained by
  `Agent`; preserve ordering between clipboard, tray, and notification events.
- macOS, Windows, and Linux are supported targets. Each target must fail
  explicitly when its required native capabilities are unavailable.
- Portable contracts live in `crates/clipboard`, `crates/notifications`, and
  `crates/tray`; native implementations and host adapters belong in their
  platform modules.

## Plugins and generated artifacts

- `crates/plugin-api/src/lib.rs` is runtime-neutral.
  `crates/runtime/src/plugins/runtime.rs` alone adapts it to Extism.
- Plugins reach the core through `ExternalRuleProvider`; keep
  `crates/core` free of plugin and WASM runtime dependencies.
- Plugin permissions are host-owned and effective capabilities are
  `requested ∩ granted`. A broken or rejected plugin degrades to structured
  issues instead of blocking application startup.
- `plugins/plugin-api-v1.xtp.yaml`, `plugins/manifest.schema.json`, and tray
  icon outputs are generated and committed. Regenerate them through the
  corresponding `just` recipe; never edit them by hand or generate them from a
  build script.
- The plugin-author contract is `plugins/API.md`. Build the example guest with
  `just build-example-plugin`; use `just test-plugins` for runtime coverage.

## Verification

- Run `just ci` for behavior changes. It mirrors the normal CI checks.
- Config-loader changes need parsing and source-tracking coverage in
  `crates/runtime/tests/config_rules.rs`.
- Platform implementation changes also need `just check-cross` when the
  required local toolchains are available.
- Run `just test-plugins` for plugin-runtime changes.
- For documentation-only work, inspect the relevant source paths, run the
  configured Markdown checks, and state that Rust tests were not needed.
- Do not weaken runtime checks to make tests or packaging pass.
