# Trusted Shell Rules

Shell rules execute with the user's authority and require:

```yaml
config:
  shell:
    enabled: true
```

Selected-text example:

```yaml
- type: shell
  id: uppercase
  shell: bash
  timeout: 5s
  run: |
    set -euo pipefail
    tr '[:lower:]' '[:upper:]'
```

Exactly one of `run` or `script_path` is required. Relative `script_path`
values resolve from the declaring config/import file.

- exit `0`: UTF-8 stdout is the replacement;
- exit `3`: no match;
- any other exit or timeout: keep input and report an error.

`item-shell` exchanges a complete item through `CT_INPUT_ITEM` and
`CT_OUTPUT_ITEM` and supports `no-match`, `replace-text`, and `replace-item`.
Use `formats: ["*"]` only when every representation is needed.

Never enable shell execution or change user-owned permissions without explicit
authorization. Remote shell imports additionally require
`config.shell.remote_imports: true`, importing-edge
`permissions.shell: true`, and a matching 64-digit SHA-256 pin. See
`imports.md` for the expanded import form.
