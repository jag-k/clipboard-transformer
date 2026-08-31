# Rulesets

A `ruleset` groups a non-empty nested `rules` list and controls how its
children compose.

```yaml
- type: ruleset
  id: normalize-example
  mode: while-matching
  formats: [text]
  rules:
    - id: upgrade-example
      from: '^http://example\.com'
      to: 'https://example.com'
    - id: remove-www
      from: '^https://www\.example\.com'
      to: 'https://example.com'
```

| Mode | Behavior |
| --- | --- |
| `all-matching` | Default. Run children in order, carry successful outputs forward, and skip non-matches. |
| `while-matching` | Stop at the first enabled non-match and keep changes from the matching prefix. |
| `all` | Require every enabled child to apply; otherwise produce no change. |
| `first` | Apply only the first child matching the original input. |

Temporarily disabled children are skipped. Nested rules may use any built-in or
plugin type, including another ruleset.

A ruleset's `groups` are inherited by its children; children may add their own
`groups`, and the effective membership is the union.

For a ruleset, `formats` is a presence filter rather than an input priority and
may name a binary native representation. The former `pipeline` and
`full-pipeline` mode names are invalid; use `all-matching` and `all`.
