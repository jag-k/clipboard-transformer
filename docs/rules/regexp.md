# Regular expression rules

`regexp` replaces UTF-8 text using Rust's `regex` syntax.

```yaml
- type: regexp
  id: github-blob-to-raw
  name: GitHub raw URL
  formats: [url, text]
  from: '^https://github\.com/([^/]+/[^/]+)/blob/([^/]+)/(.+)$'
  to: 'https://raw.githubusercontent.com/$1/$2/$3'
```

Required fields are `from` and `to`. `type` may be omitted because `regexp` is
the default.

## Patterns and flags

Rust regexes do not support look-around, pattern backreferences, or conditional
groups. Replacement already applies to every match, so there is no `g` flag.

Optional `flags` characters:

| Flag | Meaning |
| --- | --- |
| `i` | Case-insensitive. |
| `m` | Multi-line anchors. |
| `s` | Dot matches newline. |
| `U` | Swap greed. |
| `x` | Ignore insignificant pattern whitespace. |
| `u` | Unicode mode; enabled by default. |

## Replacements

`$1` and `$name` expand captures. Use braces before an identifier character:
`${1}_suffix`, not `$1_suffix`. `$$` writes a literal dollar sign.

An optional `message` uses the same capture-template syntax. Omit it when the
rule has no meaningful capture-based notification.
