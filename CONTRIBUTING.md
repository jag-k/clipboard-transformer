# Contributing

Thanks for looking at Clipboard Transformer. Bug reports from real desktop
sessions are as valuable as code, especially on Windows and Linux, where the
maintainer does less routine testing than on macOS.

## Setup

You need a stable Rust toolchain and [`just`](https://just.systems). The
optional hook runner is [`prek`](https://github.com/j178/prek), a drop-in
`pre-commit` replacement that reads `prek.toml`:

```sh
prek install
```

That installs a pre-commit stage (formatting, lint, file hygiene, actionlint,
shellcheck, markdownlint) and a pre-push stage (the test suite).

## Verifying a change

```sh
just ci
```

`just ci` is exactly what `.github/workflows/ci.yml` runs, in the same order:
`check-fmt`, `check`, `check-cli`, `check-wasm`, `check-clippy`,
`check-clippy-cli`, `test`. CI calls the same recipes rather than repeating
their commands, so a green `just ci` means CI agrees.

Recipes are namespaced by action; `just --list` shows them grouped. Ones you
may need beyond `just ci`:

| Recipe | When |
| --- | --- |
| `just test-plugins` | Any plugin change. Builds the example WASM plugin first, otherwise the plugin runtime tests skip themselves. |
| `just check-cross` | Any change under `crates/runtime/src/platform/`. Type-checks the Windows and Linux code from macOS. Needs `brew install mingw-w64 zig` plus the `x86_64-pc-windows-gnu` and `x86_64-unknown-linux-gnu` targets. |
| `just check-msrv` | Anything that might need a newer Rust than `rust-version` in `Cargo.toml`. |
| `just check-deny` | Adding or bumping a dependency. Checks advisories, licenses, sources, and duplicate versions; needs `cargo install cargo-deny --locked`. |
| `just gen-schemas` | Changing the plugin protocol in `crates/plugin-api`. Regenerates the committed authoring schemas. |
| `just gen-icons --force` | Changing `assets/tray.svg`. |

Generated files under `plugins/` and `assets/generated/` are committed and
covered by drift tests, so a stale artifact fails `just test` rather than
silently diverging. Never edit them by hand.

## Expectations for a pull request

- Keep changes small and direct. This codebase is still compact enough that a
  broad abstraction needs a stated payoff.
- Update the relevant user guide under [`docs/`](docs/README.md) when behavior
  changes. Keep the short `README.md` navigation and examples aligned.
  `AGENTS.md` lists which code paths back storage, imports, hot reload, and
  launch claims.
- Keep architecture records, future plans, evidence, and maintainer runbooks
  under [`.agents/`](.agents/README.md), not in user-facing documentation.
- Add tests next to the ones that already cover the area. Config loader changes
  belong in `crates/runtime/tests/config_rules.rs` and should cover both
  parsing and source tracking.
- Do not weaken a runtime check to make a test or packaging step pass. If a
  check exists to prove the real application path, keep it tied to that path.
- Add a `CHANGELOG.md` entry under `## [Unreleased]` for anything a user would
  notice. Release tooling rewrites that heading, so leave its markers alone.

`AGENTS.md` holds the longer set of repository conventions — platform
boundaries, the plugin protocol rules, path resolution. It is written for both
people and coding agents; read it before a larger change.

## Reporting bugs

Include your OS and version, and on Linux the desktop environment, session type
(X11 or Wayland), compositor, and tray host. `clipboard-transformer doctor`
prints the capability and path diagnostics that answer most of this. Logs are at
`<state_dir>/clipboard-transformer.log`; `clipboard-transformer paths` resolves
`<state_dir>` for your machine.

Please do not open a public issue for a security problem — see
[SECURITY.md](SECURITY.md).

## License

Contributions are accepted under the repository's MPL-2.0 license.
