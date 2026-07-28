# Documentation

Clipboard Transformer has a native desktop app for automatic clipboard
transformations and an optional standalone CLI for terminal workflows. Most
users should start with the desktop app.

## Desktop app (GUI)

- [Install Clipboard Transformer](install.md)
- [Configure the app](configuration.md)
- [Understand runtime behavior](runtime.md)
- [Choose and write rules](rules.md)
- [Split configuration with imports](imports.md)
- [Platform support](platforms.md)
- [Troubleshooting](troubleshooting.md)

## CLI and automation

- [CLI reference](cli.md)
- [Install the AI agent skill](agent-skill.md)

The CLI shares the desktop app's config and rule engine but is not required to
initialize or run the tray application.

## Extensions

- [Use WASM plugins](plugins.md)

## Terms

- **Clipboard item** — one copied value as seen by the app. It may contain
  several related forms at once, such as plain text, a URL, HTML, an image, or
  platform-specific data.
- **Representation** or **format** — one form inside a clipboard item. A rule's
  `formats` field chooses which forms it can use.
- **Text transform** — a rule that reads one selected text-like representation
  and produces text without taking ownership of unrelated native data.
- **Full-item transform** — an advanced transform that can inspect or replace
  the complete clipboard item, including non-text representations.
- **Ruleset** — a group that controls how nested rules are applied in order.
- **Source app** — the application from which the user copied the item; rules
  may include or exclude source apps.

## Rule reference

- [Regular expressions](rules/regexp.md)
- [Structural URLs and URL cleanup](rules/url.md)
- [Rulesets](rules/ruleset.md)
- [Trusted shell rules](rules/shell.md)

## Platform notes

- [Linux desktop sessions](linux.md)

Contributor setup is in [CONTRIBUTING.md](../CONTRIBUTING.md). The small set of
durable maintainer records and release runbooks lives under
[`.agents/`](../.agents/README.md).
