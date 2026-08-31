# Rules

Rules are applied in order. The top-level list behaves like an
`all-matching` ruleset: a matching rule passes its output to later rules, while
a non-match is skipped.

## Built-in types

| Type | Best use |
| --- | --- |
| [`regexp`](rules/regexp.md) | General UTF-8 text replacement. |
| [`url`](rules/url.md) | One structural URL transform. |
| [`url-cleanup`](rules/url.md#url-cleanup) | Query-parameter removal shorthand. |
| [`ruleset`](rules/ruleset.md) | Grouping and explicit ordered match policy. |
| [`shell`](rules/shell.md) | Trusted native selected-text script. |
| [`item-shell`](rules/shell.md#item-shell) | Trusted script that can inspect or replace the complete clipboard item. |

Installed WASM plugins add namespaced types such as
`dev.example.links/human-readable-link`.

## Common behavior

- Give every rule a unique, stable `id`; lowercase kebab case is recommended.
- `name` is the optional human-readable notification label.
- `formats` chooses eligible clipboard representations.
- `apps` plus `app_mode` restricts rules to source applications.
- `groups` adds the rule to one or more enablement groups; inherited from
  outer rulesets and import edges. A rule is active only when every group in
  its effective membership is enabled.
- A successful text transform authors portable plain text, removes stale
  text/URL/HTML/RTF views, and keeps unrelated native data.
- A non-match leaves the clipboard item untouched.

## Test a rule

Use the explicit stdin/stdout source to test an exact value:

```sh
printf '%s' 'https://example.com/?utm_source=test&id=42' |
  clipboard-transformer transform - --input-format url
```

Test an expected match, a no-match, and ordering when several rules interact.
Use `clipboard-transformer transform --preview` to test the current clipboard
without writing the result.
Use `clipboard-transformer rules view effective` to inspect the normalized,
import-expanded tree and source locations.
