# Imports

```yaml
rules:
  - import: rules/youtube.yaml
  - import: https://example.com/rules.toml
```

Imports accept relative/absolute paths, `file:` URLs, and HTTP(S) URLs. Relative
paths resolve from the importing file. YAML and TOML may import each other.
Extensionless content is tried as YAML and then TOML. Only rules are imported;
an imported `config` section is ignored.

The short string form is enough for ordinary imports. The expanded form carries
the trust grant and required pin for remote shell code:

```yaml
rules:
  - import:
      source: https://example.com/trusted-rules.yaml
      permissions:
        shell: true
      sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
```

Known page URLs from GitHub files and Gists, GitLab files and snippets,
Pastebin, Rentry, Hastebin, dpaste.org, Bitbucket, and Codeberg/Gitea are
normalized to their raw download form. Direct paste.rs, 0x0.st, and ttm.sh
links are treated as ordinary URL imports.

Remote imports are cached under `<state_dir>/url-imports/`. Validation may
refresh them and therefore require network access. Do not edit cache files as
authored source.
