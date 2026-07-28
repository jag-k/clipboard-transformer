# AI agent skill

The repository ships one portable Agent Skill:
`clipboard-transformer-rules`. It teaches coding agents how to locate, edit,
validate, test, and organize Clipboard Transformer YAML/TOML rules without
assuming the CLI is installed.

## Install with `npx skills`

List the discoverable skills:

```sh
npx skills add jag-k/clipboard-transformer --list
```

Install interactively for detected agents:

```sh
npx skills add jag-k/clipboard-transformer \
  --skill clipboard-transformer-rules
```

Common explicit targets:

```sh
# Codex
npx skills add jag-k/clipboard-transformer \
  --skill clipboard-transformer-rules --agent codex

# Claude Code
npx skills add jag-k/clipboard-transformer \
  --skill clipboard-transformer-rules --agent claude-code

# Several agents sharing one project
npx skills add jag-k/clipboard-transformer \
  --skill clipboard-transformer-rules \
  --agent cursor \
  --agent gemini-cli \
  --agent github-copilot \
  --agent opencode
```

Add `--global` to install for the current user instead of the current project,
or `--all` to install every discovered skill to every supported agent.

The installer chooses each agent's current directory convention and normally
uses symlinks to keep one canonical copy. The repository therefore does not
commit separate `.claude/skills`, `.cursor/skills`, `.codex/skills`, and
similar copies that would drift.

## Codex

Codex automatically discovers repository skills under `.agents/skills`, so the
skill is already available while working inside this checkout:

```text
$clipboard-transformer-rules
```

OpenAI-specific UI metadata lives beside the skill in
[`agents/openai.yaml`](../.agents/skills/clipboard-transformer-rules/agents/openai.yaml).
It provides the display name, short description, and default prompt. A root
`.openai/skills` or `.codex/skills` duplicate is not required.

## Claude Code plugin

Claude Code can use the same skill as a marketplace plugin:

```text
/plugin marketplace add jag-k/clipboard-transformer
/plugin install clipboard-transformer-rules@clipboard-transformer
```

The marketplace definition is
[`.claude-plugin/marketplace.json`](../.claude-plugin/marketplace.json). It
references the canonical `.agents/skills` folder and exposes the namespaced
skill `/clipboard-transformer-rules:clipboard-transformer-rules`.

For local development, add the checkout as the marketplace source before
installing:

```text
/plugin marketplace add .
/plugin install clipboard-transformer-rules@clipboard-transformer
```

## Skill source

- [Workflow instructions](../.agents/skills/clipboard-transformer-rules/SKILL.md)
- References:
  [paths](../.agents/skills/clipboard-transformer-rules/references/paths.md),
  [built-in rules](../.agents/skills/clipboard-transformer-rules/references/rules.md),
  [shell rules](../.agents/skills/clipboard-transformer-rules/references/shell-rules.md),
  [imports](../.agents/skills/clipboard-transformer-rules/references/imports.md),
  [plugins](../.agents/skills/clipboard-transformer-rules/references/plugins.md),
  and [CLI](../.agents/skills/clipboard-transformer-rules/references/cli.md).

The skill is intentionally self-contained so an installation copied outside
this repository retains its rule, path, and CLI references.
