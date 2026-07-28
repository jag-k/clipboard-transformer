# Clipboard Transformer

_A native desktop app that quietly cleans copied text and links using rules you
control._

[![CI](https://github.com/jag-k/clipboard-transformer/actions/workflows/ci.yml/badge.svg)](https://github.com/jag-k/clipboard-transformer/actions/workflows/ci.yml)
[![MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](LICENSE)
[![Agent skill installs](https://skills.sh/b/jag-k/clipboard-transformer)](https://skills.sh/jag-k/clipboard-transformer)

Clipboard Transformer can remove tracking parameters, normalize URLs, rewrite
text, and run ordered groups of rules. The desktop app handles the clipboard
automatically; the separate CLI is available for terminals, scripts, and
diagnostics.

## Install

The project is preparing its first public release. No GitHub Release or package
manager entry is published yet, so the current route is building from source.

See [Installation](docs/install.md) for source builds and the planned macOS,
Windows, Linux, Homebrew, WinGet, Scoop, and AUR packages.

## Desktop app (GUI)

The desktop app is the recommended way to use Clipboard Transformer. Launch it
from Applications, the Start menu, or your desktop launcher; no terminal setup
is required.

On first launch it creates a YAML config and editor schema automatically. It
then lives in the tray and provides:

- automatic clipboard transformations;
- pause and resume;
- recent transformations and Undo;
- config and plugin reload;
- open or reveal config;
- autostart and diagnostics.

There is intentionally no large settings window. Most everyday customization
is a small edit to the generated `config.yaml`.

### A small YAML config

```yaml
# $schema: ./clipboard-transformer.schema.json

config:
  recent_items_count: 5

rules:
  - type: url-cleanup
    id: remove-tracking
    name: Remove tracking parameters
    formats: [url, text]
    remove_query_prefixes: [utm_]
    remove_query_params: [fbclid, gclid]

  - type: regexp
    id: youtube-shorts
    name: Normalize YouTube Shorts
    formats: [url, text]
    from: '^https://www\.youtube\.com/shorts/([A-Za-z0-9_-]{11})$'
    to: 'https://youtu.be/$1'
```

Each rule has an `id` and a `type`; the remaining fields describe when and how
it transforms a copied value. The generated schema provides completion and
typo checking in compatible editors.

Read [Configuration](docs/configuration.md) and the
[Rule guide](docs/rules.md) when you want to add or organize rules.

### More rule examples

As a bonus, you can browse
[the maintainer's personal rules gist](https://gist.github.com/jag-k/3f9cd197776ab5db24150f5cd23026ea).
Copy only the rules you understand, or import the hosted file directly:

```yaml
rules:
  - import: https://gist.github.com/jag-k/3f9cd197776ab5db24150f5cd23026ea/raw/rules.yaml
```

A remote import can change when its owner updates it. Review the gist before
using it; for a stable personal setup, copy the rules locally or pin a specific
gist revision. See [Imports](docs/imports.md).

## Command-line interface (CLI)

The standalone `clipboard-transformer` command is optional. It shares the same
config and rules but does not own the tray, autostart, notifications, or
continuous clipboard transformation.

Use it for explicit terminal workflows:

```sh
# Find files and diagnose native capabilities.
clipboard-transformer paths
clipboard-transformer doctor

# Validate the active config.
clipboard-transformer config check

# Discover rule types available to the active config.
clipboard-transformer rules list --available-only

# Preview the current clipboard without writing the result.
clipboard-transformer transform --preview

# Transform the current clipboard once.
clipboard-transformer transform

# Use the rule engine as a stdin/stdout filter.
printf '%s' 'https://example.com/?utm_source=test&id=42' |
  clipboard-transformer transform -
```

The CLI never starts a hidden daemon. See the complete
[CLI reference](docs/cli.md).

## Rules

| Type | Purpose |
| --- | --- |
| [`regexp`](docs/rules/regexp.md) | Text matching and replacement. |
| [`url`](docs/rules/url.md) | Encoding-safe structural URL changes. |
| [`url-cleanup`](docs/rules/url.md#url-cleanup) | Query-parameter cleanup. |
| [`ruleset`](docs/rules/ruleset.md) | Ordered composition and match policies. |
| [`shell`](docs/rules/shell.md) | Explicitly trusted local scripts. |
| `<plugin-id>/<rule-type>` | Sandboxed WASM extensions. |

Rules may be limited by clipboard format or source application. YAML and TOML
configs may also split rule sets across local or remote imports.

## Agent skill

The repository includes an installable `clipboard-transformer-rules` skill for
agents that help write and validate configs:

```sh
npx skills add jag-k/clipboard-transformer \
  --skill clipboard-transformer-rules
```

It has one canonical copy under
[`.agents/skills/clipboard-transformer-rules`](.agents/skills/clipboard-transformer-rules/SKILL.md).
See [Agent setup](docs/agent-skill.md) for Codex, Claude Code, Cursor, Gemini
CLI, GitHub Copilot, and OpenCode.

## Support and development

macOS, Windows, and Linux are supported targets. See
[Platform support](docs/platforms.md), the detailed
[Linux guide](docs/linux.md), and [Troubleshooting](docs/troubleshooting.md).

The [documentation index](docs/README.md) links the full user guide.
Contributors should read [CONTRIBUTING.md](CONTRIBUTING.md); active work is
tracked in [TODO.md](TODO.md).

Security reports belong in [SECURITY.md](SECURITY.md). The project is licensed
under [MPL-2.0](LICENSE).
