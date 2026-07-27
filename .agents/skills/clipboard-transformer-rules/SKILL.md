---
name: clipboard-transformer-rules
description: Locate, write, edit, validate, test, and organize Clipboard Transformer YAML or TOML rules. Use for clipboard rewrites, URL cleanup, regular expressions, rulesets, app/format filters, local or remote imports, trusted shell rules, WASM plugin rules, config diagnostics, effective-rule inspection, and rule-focused CLI workflows, whether or not the clipboard-transformer CLI is installed.
---

# Clipboard Transformer Rules

Make the smallest correct config change. Preserve comments, ordering,
formatting style, imports, and unrelated user settings. Prefer application-level
validation, but keep discovery and editing usable without an installed CLI.

Load only the references needed for the task:

- Read `references/paths.md` when locating config, state, cache, schema, or
  plugin paths.
- Read `references/rules.md` before authoring shared fields, built-in text/URL
  rules, or rulesets.
- Read `references/shell-rules.md` before adding or changing `shell` or
  `item-shell` rules.
- Read `references/imports.md` when inspecting, adding, or changing imports.
- Read `references/plugins.md` for plugin-owned rule types or plugin commands.
- Read `references/cli.md` before constructing validation, rule-catalog,
  effective-view, transform, watch, or diagnostic commands.

## Workflow

1. Locate the active config.
   - Use a user-supplied path when present.
   - Otherwise run `clipboard-transformer paths` when available.
   - In a repository checkout, prefer an existing
     `target/release/clipboard-transformer`, then use
     `cargo run --locked -- paths` when practical.
   - Without a CLI, resolve platform and XDG paths from `references/paths.md`
     and check both `config.yaml` and `config.toml`.
   - Remember that launching the desktop app creates the starter YAML and
     schema when neither format exists; CLI initialization is optional.
   - If neither exists, create `config.yaml` directly only when the user
     authorized a new config. Ask only when multiple existing configs remain
     plausible.

2. Inspect the main config and relevant imports.
   - Classify inline rules, rulesets, local imports, remote imports, shell rules,
     plugin rules, or a mixture.
   - Open relevant local imports. Treat remote imports as external dependencies,
     not casual edit targets.
   - Do not change plugin permissions or global shell authorization merely to
     make a proposed rule pass.

3. Select the narrowest rule shape.
   - Use `url-cleanup` for simple HTTP(S) query-parameter removal.
   - Use `url` for one structural URL operation.
   - Use `regexp` for UTF-8 text rewrites and capture replacements.
   - Use `ruleset` only for grouping or ordered match policy.
   - Use `shell` or `item-shell` only when the user explicitly needs trusted
     native execution and the portable built-ins cannot express the behavior.
   - For namespaced plugin types, follow `references/plugins.md` and inspect
     that plugin's example, manifest, schema, or existing rules. Never guess
     plugin-owned fields.
   - Use `clipboard-transformer rules list` to discover known built-in, native,
     and plugin rule types. Add `--available-only` when current config and
     plugin availability should filter the catalog.

4. Edit conservatively.
   - Give every real rule a stable non-empty id; prefer lowercase kebab case.
   - Prefer single-quoted YAML strings for regexes and replacements.
   - Omit `formats` when `[text]` is correct. Otherwise preserve priority.
   - Pair non-empty `apps` with `app_mode: blacklist` or `whitelist`.
   - Treat patterns as Rust regexes, not PCRE. Avoid look-around, pattern
     backreferences, conditional groups, and overly broad prose rewrites.
   - Keep one `transform` per `url` rule; compose operations with a ruleset.

5. Validate with the strongest available method.
   - Prefer `clipboard-transformer config check --config-file <path>`, then
     pipe exact samples through
     `clipboard-transformer transform - --config-file <path>`. Add
     `--input-format <format>` when the rule does not select plain text.
   - In a checkout, use the existing release binary or the same subcommands
     through `cargo run --locked --`.
   - For import work, also inspect
     `clipboard-transformer rules view effective --config-file <path>`.
   - The explicit `-` selects stdin/stdout:
     `transform - --config-file <path>`. Without it, `transform` reads and
     writes the current clipboard; `--preview` suppresses that write.
   - Across rule-loading commands, `--config-file` always means a path and
     `--config` always means a self-contained inline YAML or TOML document.
   - Use `config schema --output <path>` only when the user asks for a separate
     schema artifact. Normal desktop startup maintains the schema beside the
     active config.
   - When no executable is available, perform structural review against the
     relevant references and the generated schema beside the config. State
     that import refresh, plugin initialization, and rule compilation were not
     verified.

6. Test behavior.
   - Use the rule's actual selected format instead of always choosing `text`.
   - Test an expected match, a no-match, and ordering for rulesets.
   - For shell rules, test exit `0`, no-match exit `3`, stderr diagnostics,
     timeout behavior when relevant, and avoid using live sensitive clipboard
     content.

7. Report the changed file, rule id, validation method, and observed results.
   Mention any unavailable app-level verification without implying the CLI had
   to be installed.

## Editing rules

- Add a rule near related inline rules, inside a fitting existing ruleset, or
  in an established local import—whichever creates the least churn.
- Create a new import only when it improves an already modular or crowded
  config.
- Keep relative imports relative to the importing file.
- Do not edit cached URL-import files as authored source.
- Runtime parsing ignores unknown fields; generated JSON Schema is stricter.
  Do not treat runtime validation alone as typo detection.
- For URL cleanup, prefer exact names, then prefixes, then regex patterns.
- Never weaken SHA-256 pins or remote shell permissions to make validation pass.

## References

- `references/paths.md`: config discovery and platform/XDG path resolution.
- `references/rules.md`: shared fields and built-in regexp, URL, cleanup, and
  ruleset semantics.
- `references/shell-rules.md`: executable rule behavior and trust boundaries.
- `references/imports.md`: local/remote import resolution, caching, and pins.
- `references/plugins.md`: plugin rule discovery and lifecycle commands.
- `references/cli.md`: rule-focused commands, config-source contracts, and
  validation scope.
