# Built-in Rules

## Contents

- Shared configuration
- Regexp
- Structural URL
- URL cleanup
- Rulesets

## Shared configuration

```yaml
# $schema: ./clipboard-transformer.schema.json

config:
  double_copy_window: 10
  disable_for: 600
  import_refresh_interval: 600
  apps: []

rules:
  - type: regexp
    id: example
    from: '^old$'
    to: 'new'
```

Every real rule requires a non-empty `id`. Shared optional fields:

- `name`: display label, falling back to `id`;
- `formats`: ordered input priority for text transforms, or presence filter for
  rulesets;
- `apps`: source app bundle ids, executable names, or display names;
- `app_mode`: required with non-empty `apps`; `blacklist` skips matches and
  `whitelist` permits only matches.

An omitted or empty `formats` defaults to `[text]`. Portable aliases include
`text`/`plain-text`, `url`, `html`, `rtf`, and `file`/`file-url`; exact native
identifiers are allowed.

Core built-in types are `regexp`, `url`, `url-cleanup`, and `ruleset`. The
native host adds `shell` and `item-shell`; plugins add
`<plugin-id>/<rule-type>`. Use `clipboard-transformer rules list` for the
runtime catalog and `--available-only` to filter it by current availability.

## Regexp

Required: `from`, `to`. `regexp` is the only built-in whose `type` may be
omitted; all other built-ins require an explicit type.

```yaml
- type: regexp
  id: github-blob-to-raw
  formats: [url, text]
  from: '^https://github\.com/([^/]+/[^/]+)/blob/([^/]+)/(.+)$'
  to: 'https://raw.githubusercontent.com/$1/$2/$3'
```

Patterns use Rust `regex`. Unsupported: look-around, pattern backreferences,
and conditional groups. Replacement is global; there is no `g` flag.

Optional flags: `i`, `m`, `s`, `U`, `x`, and `u`. Replacements and regexp
messages expand `$1`, `$name`, and `${1}_suffix`; `$$` writes a literal dollar.

## Structural URL

A `url` rule applies to a complete whitespace-free HTTP(S) URL and contains
one `transform`:

```yaml
- type: url
  id: remove-fragment
  formats: [url, text]
  transform:
    type: remove-components
    components: [fragment]
```

Optional `hosts` are exact case-insensitive host names. Operations:

- `remove-query-params`: `names`, `prefixes`, `patterns`;
- `remove-components`: any of `fragment`, `query`, `credentials`, `port`,
  or `path` (reset to `/`);
- `rewrite-host`: `to`, optional `from`;
- `rewrite-scheme`: `to` (`http` or `https`), optional `from`;
- `set-query-param`: `name`, `value`.

Compose several operations with an `all-matching` ruleset rather than
inventing a transform list.

## URL cleanup

`url-cleanup` is a query-removal shorthand:

```yaml
- type: url-cleanup
  id: remove-tracking
  formats: [url, text]
  remove_query_params: [fbclid, gclid]
  remove_query_prefixes: [utm_]
  remove_query_param_patterns:
    - 'ga_[a-z_]+'
```

At least one removal field must be non-empty. Exact names, prefixes, and
anchored patterns match case-insensitively. Kept segments preserve raw
encoding. The rule applies only when it removes something.

## Rulesets

```yaml
- type: ruleset
  id: normalize-example
  mode: while-matching
  rules:
    - id: upgrade
      from: '^http://example\.com'
      to: 'https://example.com'
    - id: remove-www
      from: '^https://www\.example\.com'
      to: 'https://example.com'
```

Modes:

- `all-matching` (default): carry successes and skip non-matches;
- `while-matching`: stop at the first enabled non-match and keep the matching
  prefix;
- `all`: every enabled child must apply or the ruleset produces no change;
- `first`: apply the first child matching the original input.

Disabled children are skipped. `pipeline` and `full-pipeline` are invalid old
names; migrate them to `all-matching` and `all`.
