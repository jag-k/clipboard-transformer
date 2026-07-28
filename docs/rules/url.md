# URL rules

Structural URL rules operate on a complete, whitespace-free HTTP(S) URL and
preserve the raw encoding of untouched components.

Each `url` rule contains exactly one `transform`. Compose several operations
with a [`ruleset`](ruleset.md).

```yaml
- type: ruleset
  id: canonical-example-url
  mode: all-matching
  formats: [url, text]
  rules:
    - type: url
      id: remove-tracking
      transform:
        type: remove-query-params
        prefixes: [utm_]
        names: [fbclid]
    - type: url
      id: remove-fragment
      transform:
        type: remove-components
        components: [fragment]
```

Optional `hosts` values match exact host names case-insensitively; subdomains
are not implied.

## Transforms

| Transform | Fields | Behavior |
| --- | --- | --- |
| `remove-query-params` | `names`, `prefixes`, `patterns` | Remove matching query segments. At least one selector is required. |
| `remove-components` | `components` | Remove `fragment`, `query`, `credentials`, or `port`; `path` resets to `/`. |
| `rewrite-host` | `to`, optional `from` | Replace the host while retaining other components. |
| `rewrite-scheme` | `to`, optional `from` | Replace the scheme with `http` or `https`. |
| `set-query-param` | `name`, `value` | Replace exact occurrences and append one encoded pair. |

Names, prefixes, and anchored patterns in `remove-query-params` are
case-insensitive. A `rewrite-host.to` value is a bare host without a port.

## URL cleanup

`url-cleanup` is a supported shorthand for removing query parameters:

```yaml
- type: url-cleanup
  id: remove-tracking
  formats: [url, text]
  hosts: [example.com]
  remove_query_params: [fbclid, gclid]
  remove_query_prefixes: [utm_]
  remove_query_param_patterns:
    - 'ga_[a-z_]+'
```

At least one removal field must be non-empty. Exact names, prefixes, and
patterns match case-insensitively. The rule applies only when it removes a
parameter.
