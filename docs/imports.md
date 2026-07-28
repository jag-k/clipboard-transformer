# Imports

Imports split rules across local YAML/TOML files or reuse remote rule sets.
They are entries in `rules`, not normal rules:

```yaml
rules:
  - import: rules/youtube.yaml
  - import: https://example.com/shared-rules.toml
```

TOML:

```toml
[[rules]]
import = "rules/youtube.toml"
```

Only rules are imported. An imported top-level `config` section is ignored, so
application settings always come from the main config.

## Sources and formats

Imports accept:

- paths relative to the importing file;
- absolute paths;
- `file:` URLs;
- `http:` and `https:` URLs.

YAML and TOML may import each other. `.yaml`, `.yml`, and `.toml` choose the
parser; extensionless content is tried as YAML and then TOML. Imported YAML may
be a rule list or a mapping containing `rules`.

Remote imports are cached under `<state_dir>/url-imports/`. Common GitHub,
GitLab, Gist, paste, Bitbucket, and Codeberg/Gitea browser URLs are normalized
to raw downloads.

## Trust

Treat remote imports as external dependencies. Review their source and pin
remote executable shell rules with `sha256`; see
[trusted shell imports](rules/shell.md#imported-shell-rules).

Validation expands imports and reports cycles, duplicate flattened ids, source
locations, and malformed entries:

```sh
clipboard-transformer config check
clipboard-transformer rules view effective
```
