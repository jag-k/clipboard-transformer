# Trusted shell rules

Shell rules execute native processes with the user's authority. They are
disabled by default, unavailable in portable browser/WASM hosts, and should be
used only for code you trust.

Enable them explicitly:

```yaml
config:
  shell:
    enabled: true
```

## Shell

`shell` exchanges the selected UTF-8 value through stdin/stdout:

```yaml
rules:
  - type: shell
    id: uppercase
    shell: bash
    timeout: 5s
    run: |
      set -euo pipefail
      tr '[:lower:]' '[:upper:]'
```

Exactly one of `run` or `script_path` is required. Relative script paths are
resolved from the config or imported file declaring the rule.

Exit codes:

| Code | Result |
| ---: | --- |
| `0` | Replace with UTF-8 stdout. Empty output is valid. |
| `3` | No match. |
| other | Keep the clipboard unchanged and report a rule error. |

The process is non-interactive and receives `CT_CONFIG_DIR`, `CT_STATE_DIR`,
`CT_CACHE_DIR`, `CT_RULE_ID`, `CT_INPUT_FORMAT`, and best-effort source-app
metadata. Rule-local `env` values may add variables but cannot override
reserved `CT_*` or `PWD`.

## Item shell

`item-shell` is the advanced full-item form. Instead of receiving only selected
text, it exchanges the complete clipboard item through JSON files named by
`CT_INPUT_ITEM` and `CT_OUTPUT_ITEM`. Output actions are:

- `no-match`;
- `replace-text`;
- `replace-item`.

Use `formats: ["*"]` only when the script genuinely needs every native
representation; it increases clipboard reads and may expose sensitive payloads
to the script.

### Input item

The input is a versioned exchange document rather than the application's
private persisted shape:

```json
{
  "version": 1,
  "platform": "macos",
  "source_app": {
    "bundle_id": "com.apple.Safari",
    "name": "Safari"
  },
  "semantics": {
    "text": {
      "value": "https://example.com/",
      "authored": false,
      "derived_from": ["public.utf8-plain-text"]
    }
  },
  "representations": [
    {
      "kind": "public.utf8-plain-text",
      "encoding": "utf8",
      "data": "https://example.com/"
    },
    {
      "kind": "public.png",
      "encoding": "file",
      "path": "/absolute/temp/path/input/representations/001.bin"
    }
  ]
}
```

Portable semantics are JSON strings. Valid UTF-8 native bytes use
`encoding: "utf8"`; non-UTF-8 input uses an absolute temporary file path.

### Output actions

No match:

```json
{ "action": "no-match" }
```

Portable text replacement:

```json
{
  "action": "replace-text",
  "text": "new text",
  "message": "Optional notification"
}
```

Atomic full-item replacement:

```json
{
  "action": "replace-item",
  "item": {
    "version": 1,
    "platform": "portable",
    "semantics": {
      "text": {
        "value": "new text",
        "authored": true
      },
      "html": {
        "value": "<strong>new text</strong>",
        "authored": true
      }
    },
    "representations": []
  }
}
```

Output native representations may use `utf8`, `file`, or `base64`. Referenced
files must be regular non-symlink files inside the invocation's output
directory. The host rejects traversal, symlink escapes, outside paths, and
oversized payloads, then removes the temporary exchange directory.

## Imported shell rules

Local imported shell rules are allowed by default once shell support is
enabled. Remote imports are denied unless all of these are explicit:

```yaml
config:
  shell:
    enabled: true
    remote_imports: true

rules:
  - import:
      source: https://example.com/trusted-rules.yaml
      permissions:
        shell: true
      sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
```

The digest pins the downloaded bytes. A mismatch fails closed.
