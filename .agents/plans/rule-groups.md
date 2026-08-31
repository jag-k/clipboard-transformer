# Rule groups and shared group state

Status: implementation contract for the rule-groups feature. The configuration,
runtime, CLI, and desktop work is present on the feature branch; this document
remains the detailed source of truth until release documentation supersedes it.

## Goals

Rule groups provide a compact, user-controlled way to enable and disable related
rules across the desktop app, CLI, and future SDKs. They are not execution
containers: existing `ruleset` rules retain sole ownership of ordering and
match-policy semantics.

The design must support large and imported rule collections without allowing an
imported set with many internal labels to flood the tray UI. Configuration
remains declarative and suitable for sharing; mutable enablement is local state.

## Model

A group has three distinct concerns:

- **membership**: an optional `groups: [group-id, ...]` field on a rule,
  ruleset, or rule import entry;
- **activation**: whether membership participates in deciding whether a rule is
  active;
- **presentation**: an optional root-level descriptor that supplies a label,
  description, and tray visibility.

Group IDs are stable strings. A rule may belong to several groups. A rule is
active only when every group in its effective membership is active. Rules with
no groups are unaffected.

Effective membership is an unordered set: duplicate IDs from the rule, its
rulesets, and its import edges collapse into one, and ID order carries no
semantic meaning. Order is retained only as diagnostic provenance (which
source contributed which ID). Configuration syntax still uses lists; the
loader performs the deduplication.

Group IDs share one flat global namespace across the root config and all
imports. This is deliberate: it is what lets a package expose groups as part
of its interface, and it avoids proliferating near-duplicate labels. The cost
is that unrelated sources can collide on an ID and become coupled through the
shared state document. Documentation should recommend that published packages
namespace their exported IDs (for example `@url-cleaner/privacy`, in the style
of scoped package names), so the ID grammar must permit `@` and `/`.
Namespacing is a convention, not a guarantee. Diagnostics may warn when two
unrelated import sources contribute the same group ID without a root
descriptor, which smells accidental; a root descriptor signals deliberate
adoption.

A `ruleset` is not a group. Its `mode` continues to determine ordered execution
of children. A ruleset's group membership is inherited by its descendants;
children may add their own membership. Membership is additive only: there is
no syntax to subtract an inherited group.

## Configuration shape

The proposed root-level catalog is optional and is presentation metadata, not a
required registry:

```yaml
groups:
  privacy:
    name: Privacy
    description: Removes tracking parameters and advertising identifiers
    status: visible

  vendor-internal:
    status: hidden

  obsolete-vendor-label:
    status: ignore

rules:
  - type: url-cleanup
    id: remove-tracking
    groups: [privacy]
    remove_query_prefixes: [utm_]
```

An undeclared group is valid. It has no description, uses its ID as its fallback
label in diagnostics, is active by default, and is not shown in the tray.

A single `status` field captures the meaningful combinations:

- `status: visible` — the group is functional and the desktop tray exposes it
  as a switch;
- `status: hidden` — the group is functional but never shown in the tray; it
  remains controllable through the CLI and the state document;
- `status: ignore` — the label is removed from effective membership and ceases
  to exist for evaluation purposes.

Earlier revisions split this into separate `visibility` and `policy` fields on
the assumption that the two concerns were orthogonal. They are not: an ignored
group has no honest representation, so `visible` + `ignore` was either invalid
or indistinguishable from `hidden` + `ignore`, and per-field precedence
merging across a root descriptor and imported descriptors could manufacture
that invalid combination from two individually valid ones. One atomic status
with whole-value override avoids both problems.

`disabled` is intentionally not a configuration status. It describes mutable
user state and belongs in the group state file. `status: ignore` instead means
that this group label is removed from effective membership, regardless of any
state entry: the group ceases to exist for evaluation purposes, so the tray
never renders it and the CLI refuses to enable or disable it — any state entry
for it is inert.

For locally declared descriptors, the anticipated default status is `visible`.
Imported descriptors default to `hidden` unless the importing root config
explicitly exposes them. The final schema must make these
provenance-dependent defaults clear.

## Shared local state

The active group switches live in `<state-dir>/groups.json`, separate from the
desktop application's history, pause, and temporary per-rule state. Desktop and
CLI use the standard state path by default, so their effective rule sets agree
without editing user-authored YAML or TOML.

Reserve an extensible state document from the beginning, even though initial
implementation only needs persistent enable/disable:

```json
{
  "version": 1,
  "groups": {
    "privacy": { "enabled": false },
    "experimental": { "enabled": true }
  }
}
```

An absent group entry means enabled. Writers may remove an explicit
`enabled: true` entry as canonicalization. Future versions may add fields such
as expiry, change time, or a reason inside each group object; none of those
features are implied by this plan.

Malformed state is never used for rule evaluation. The CLI fails read-only
commands with a human-readable error that names the concrete file and suggests
fixing it, deleting it to reset all groups to enabled, or passing
`--ignore-group-state`. Explicit writers — `groups enable`/`groups disable` and
desktop tray toggles — treat the state file as app-owned and rewrite it from the
last known-good in-memory snapshot, or from an empty state when no snapshot has
been loaded, logging the parse error. The desktop keeps its last known-good
in-memory state for runtime; if startup has no valid state, grouped rules remain
disabled until the file is repaired or an explicit toggle rewrites it.

State must never be supplied by a configuration import. A remote or local rule
package may describe a group but cannot change whether it is enabled on a
machine.

The CLI contract should include:

```text
--ignore-group-state       Use configuration defaults only.
--group-state <path>       Use an explicit group-state document.
```

The flags are mutually exclusive. Future explicit commands such as `groups
list`, `groups enable <id>`, and `groups disable <id>` operate on the same state
document. They do not start a background service or rewrite the config.

CLI writers must update the file atomically. The desktop app watches this file
with a small dedicated watcher — separate from the config reloader, whose
pipeline and target filtering are built for full reloads — and posts a group
state change into the regular app command channel. The application keeps the
rule-ID-to-groups membership map computed at config load and, on a state
change, only re-reads the state document and recomputes the disabled-rule set.
A state update must not require a full config reload and must still apply
while the config itself is temporarily broken.

The default state scope is one global user profile at the resolved state path.
Automation and tests may select an isolated document with `--group-state`; the
initial design does not namespace state by config file.

## Imports

### Rule imports

A rule import may add membership to every rule in its imported subtree:

```yaml
rules:
  - import:
      source: https://example.com/url-cleaner.yaml
      groups: [url-cleaner]
      ignore_imported_groups: true
```

`groups` on the import entry is inherited by imported rules, including nested
rulesets and nested imports. It is additive to membership inherited from parent
rulesets and import entries.

`ignore_imported_groups` accepts `true` or a list of group IDs. `true` removes
all group membership authored within the imported subtree, including its
nested imports. A list removes only the named IDs and preserves the rest, so a
mostly useful package can be imported while dropping one or two labels that
collide with local conventions. Membership inherited from the importing
document is never removed. Naming an ID the subtree does not use produces a
warning with the source path. This makes an imported package opaque — wholly
or in part — with respect to its internal labels while allowing the root
configuration to assign one local control group such as `url-cleaner`.

Without the flag, imported membership is preserved. This supports shared rule
packages where groups are deliberately part of the package's interface.

Expansion applies these annotations while expanding an import, before the
imported rules are spliced into the importing document: ignored IDs are
stripped from the whole subtree first, then the import entry's groups are
added to the subtree's root rules (their descendants inherit membership
through normal ruleset nesting). Because nested imports expand bottom-up,
stripping also removes labels added by nested import entries, which matches
the "authored within the imported subtree" scope. Expansion must retain
provenance (rule ID to source path and import chain) for diagnostics.

Repeated imports of the same source are deduplicated: the loader keeps a
single copy of each imported rule to preserve rule-ID uniqueness, which
notifications, undo, temporary disables, and history rely on. Import-entry
groups from every import edge targeting the same source merge onto that single
copy. Activation remains conjunctive, so a rule reachable through edges
annotated `groups: [a]` and `groups: [b]` is active only when both `a` and `b`
are active. The merge is order-independent; diagnostics should still name each
contributing import edge.

### Group metadata imports

Rules need positional imports to preserve execution order. Group descriptors do
not, so they use a separate root-level mechanism rather than overloading the
`groups` map with a reserved `import` key:

```yaml
group_imports:
  - source: ./shared-rules.yaml
    status: hidden

rules:
  - import: ./shared-rules.yaml
```

A group metadata import reads only the source document's top-level `groups`
section. It does not import `config`, `plugins`, mutable state, or rules. The
same source can therefore supply group descriptions via `group_imports` and
rules through the existing positional rule import.

Imported group descriptors default to hidden. An import-level status may
provide a default for descriptors from that source; a root descriptor always
wins, replacing the imported status atomically. Proposed descriptor precedence
is:

```text
root `groups.<id>`
    > later `group_imports` entry
        > earlier `group_imports` entry
```

Conflicting imported descriptors should produce a warning with both source
paths. Later imports are deterministic overrides. `group_imports` is purposely
narrow. General top-level imports should only be introduced after another safe,
well-defined metadata section exists; imported `config` and `plugins` remain
out of scope because they change host-wide behaviour and trust boundaries.

## Tray and scale

The tray shows only groups whose effective descriptor has `status: visible`,
rendered as switches reflecting state. It never auto-promotes each distinct
group found in rules. Thus a package with thousands of fine-grained groups
remains usable: the root config can hide its imported metadata, omit metadata
imports entirely, or import its rules with `ignore_imported_groups: true` and
apply one local visible group.

Groups with `status: ignore` never appear regardless of any other metadata:
the status removes them from evaluation, so no honest control exists for them.

Two independent limits are required:

1. a configuration-loader safety limit on total distinct effective group IDs,
   membership entries per rule/import, and group-ID length; diagnostics must
   include the relevant source and import chain;
2. a desktop tray limit on visible groups only. Exceeding it must leave runtime
   semantics unchanged, emit a clear diagnostic, and offer an `Open config`
   path rather than silently pretending omitted controls do not exist.

Exact values should be chosen after implementation measurements and native tray
constraints are known. They are intentionally not public configuration knobs
in the first revision.

## Effective evaluation order

For every leaf rule, loading computes effective membership by collecting
groups from outer rulesets and the rule itself; import-entry annotations have
already been applied to the tree during import expansion. Deduplicate IDs
while preserving diagnostic provenance. Then:

1. discard groups whose root descriptor has `status: ignore` — this precedence
   over state is absolute;
2. read the selected group-state document unless `--ignore-group-state` was
   requested;
3. disable the rule when any remaining group has `enabled: false` in state;
4. compile and execute the unchanged rule tree, preserving existing `ruleset`
   modes and ordering.

The core rule engine should not gain a dependency on persistence, tray UI, or
config-file handling. Runtime/config code resolves group policy and state into
the existing availability/disabled-rule mechanism.

Plugin-provided rule types need no additional mechanism: plugin rules are
declared in configuration like any other rule, so `groups` applies to them
directly. Their membership counts toward the loader limits, and it cannot be
annotated or stripped by import entries, because plugin rules do not pass
through rule imports.

## Implementation sequence

1. Define portable configuration types and schema for descriptors, rule
   membership, and expanded rule imports (including the `true | [ids]` form of
   `ignore_imported_groups`); add parsing, source tracking, provenance
   tagging, and effective-membership tests. Coverage must include the
   compacted single-child ruleset chain: group-derived disables resolve to
   per-wrapper rule IDs, and that path is easy to break silently.
2. Implement the versioned group-state reader/writer, atomic mutation, state
   selection flags, malformed-state handling, and CLI inspection/mutation
   commands.
3. Apply state-derived availability in runtime and verify equivalent desktop and
   CLI transformations.
4. Add the dedicated desktop state watcher with the load-time membership map
   and a bounded tray group menu; expose only `status: visible` descriptors,
   and include group context in recent-transform diagnostics.
5. Add `group_imports`, imported-descriptor precedence diagnostics, import
   group inheritance with repeated-import merging, and
   `ignore_imported_groups`.
6. Update the user configuration/import/rule/CLI documentation and the
   `clipboard-transformer-rules` Agent Skill as each public configuration or CLI
   capability becomes implemented. Documentation must cover the flat ID
   namespace and the `@vendor/name` namespacing convention for published
   packages.

Before implementation, resolve concrete size limits, TOML representation for
`group_imports` and descriptor maps, and native tray overflow behaviour.
