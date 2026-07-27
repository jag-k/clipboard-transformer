# gitlab-link

Example [Clipboard Transformer](../../README.md) plugin. It rewrites copied
GitLab project, planning, CI/CD, and repository URLs into Markdown links:

```text
https://gitlab.example.com/acme/platform/widget/-/merge_requests/123
  -> [acme/platform/widget!123](https://gitlab.example.com/acme/platform/widget/-/merge_requests/123)
```

With a configured instance token and an `http` grant it fetches the real
resource title or project name through the host HTTP capability:

```text
  -> [Fix clipboard race (acme/platform/widget!123)](https://gitlab.example.com/...)
```

Any fetch failure — missing grant, missing token, network error, non-200 —
falls back to the offline label, so the rule always produces something.

The `project` rule also handles configured project-page aliases:

```text
https://gitlab.example.com/acme/platform/widget/-/pipelines
  -> [acme/platform/widget (Pipelines)](https://gitlab.example.com/...)
  -> [Widget (acme/platform/widget, Pipelines)](https://gitlab.example.com/...)
```

Direct MR and issue comment links retain their fragment. `comment_display`
controls their label:

```text
https://gitlab.example.com/acme/platform/widget/-/merge_requests/123#note_456
  -> [acme/platform/widget!123 (comment)](https://gitlab.example.com/...#note_456)
```

Available modes are `hidden`, `marker` (the default shown above), `id`
(`comment 456`), `author` (`comment by @user`), and `author-and-id`
(`comment 456 by @user`). Author modes use the Notes API in online mode and
fall back to the comment ID when the author cannot be fetched.

The Markdown target retains the copied URL, including its query, fragment, and
trailing slash. A specific pipeline uses its own rule and label:

```text
https://gitlab.example.com/acme/platform/widget/-/pipelines/987?ref=main#jobs
  -> [acme/platform/widget Pipeline #987](https://gitlab.example.com/acme/platform/widget/-/pipelines/987?ref=main#jobs)
```

Other individual resources use concise GitLab-style references:

```text
/-/milestones/42  -> acme/platform/widget%42
/-/jobs/654       -> acme/platform/widget Job #654
/-/commit/0123456789abcdef  -> acme/platform/widget@01234567
/-/tags/v2.1.0    -> acme/platform/widget@v2.1.0 (Tag)
```

The `repository` rule handles `tree`, `blob`, `raw`, `blame`, `commits`, and
`compare`. It displays the complete locator after the kind, because branch
names can contain slashes and cannot be reliably separated from a file path by
looking at URL segments alone:

```text
/-/blob/feature/topic/src/lib.rs
  -> acme/platform/widget (File: feature/topic/src/lib.rs)

/-/blob/main/src/lib.rs#L10
  -> acme/platform/widget (File: main/src/lib.rs, line 10)

/-/blob/main/src/lib.rs#L10-20
  -> acme/platform/widget (File: main/src/lib.rs, lines 10–20)
```

For `blob`, `raw`, and `blame` links with `ref_type=heads` or `ref_type=tags`,
online mode asks GitLab for the longest matching ref prefix. This distinguishes
a slash-bearing branch such as `feature/search-v2` from the following file
path. Commit-SHA locators are split locally. If a ref cannot be resolved, the
plugin deliberately displays the complete locator instead of guessing.

```text
/-/blob/feature/search-v2/docker/api.Dockerfile?ref_type=heads
  -> project (File: docker/api.Dockerfile, branch feature/search-v2)
```

## Build and install

Requires the `wasm32-wasip1` target (`rustup target add wasm32-wasip1`). From
this directory:

```sh
just build     # cargo build --target wasm32-wasip1 --release
just install   # build + copy gitlab_link.wasm into <config_dir>/plugins/
```

`just install` resolves the plugin directory from `clipboard-transformer
plugin paths`. A running desktop app hot-reloads the plugin automatically;
`clipboard-transformer plugin list` should then show it as `operational`.

## Configuration

Minimal (matches gitlab.com and falls back to offline labels):

```yaml
rules:
  - type: dev.jag-k.gitlab/project
    id: gitlab-project
  - type: dev.jag-k.gitlab/mr
    id: gitlab-mr
    comment_display: marker
  - type: dev.jag-k.gitlab/issue
    id: gitlab-issue
    # hidden | marker | id | author | author-and-id
    comment_display: author-and-id
  - type: dev.jag-k.gitlab/milestone
    id: gitlab-milestone
  - type: dev.jag-k.gitlab/pipeline
    id: gitlab-pipeline
  - type: dev.jag-k.gitlab/job
    id: gitlab-job
  - type: dev.jag-k.gitlab/commit
    id: gitlab-commit
  - type: dev.jag-k.gitlab/tag
    id: gitlab-tag
  - type: dev.jag-k.gitlab/repository
    id: gitlab-repository
```

Full (real titles from a self-hosted instance):

```yaml
plugins:
  dev.jag-k.gitlab:
    permissions:
      http: ["gitlab.example.com"]
      env_expansion: true
    settings:
      instances:
        - host: gitlab.example.com
          token: ${GITLAB_TOKEN}

rules:
  - type: dev.jag-k.gitlab/project
    id: gitlab-project
    # Exact paths below /-/. This map replaces the defaults when present.
    # Nested paths are supported.
    aliases:
      merge_requests: MRs
      issues: Issues
      pipelines: Pipelines
      releases: Releases
      wikis/home: Wiki
    # hosts defaults to the configured instance hosts.
    # online defaults to true; set false to force offline labels.
  - type: dev.jag-k.gitlab/mr
    id: gitlab-mr
    # hosts defaults to the configured instance hosts.
    # online defaults to true; set false to force offline labels.
  - type: dev.jag-k.gitlab/issue
    id: gitlab-issue
  - type: dev.jag-k.gitlab/milestone
    id: gitlab-milestone
  - type: dev.jag-k.gitlab/pipeline
    id: gitlab-pipeline
  - type: dev.jag-k.gitlab/job
    id: gitlab-job
  - type: dev.jag-k.gitlab/commit
    id: gitlab-commit
  - type: dev.jag-k.gitlab/tag
    id: gitlab-tag
  - type: dev.jag-k.gitlab/repository
    id: gitlab-repository
    # All are enabled by default.
    kinds: [tree, blob, raw, blame, commits, compare]
```

When `aliases` is omitted, the project rule enables common project pages:
activity, analytics, issue boards, branches, CI Lint, container registry,
deployments, environments, feature flags, forks, infrastructure, issues, jobs,
labels, merge requests, milestones, packages, pipeline schedules, pipelines,
members, releases, security dashboard, snippets, tags, and the wiki home page.
An explicit empty map (`aliases: {}`) limits it to bare project URLs. Aliases
match exact paths only, so semantic rules remain responsible for individual
resources.

`clipboard-transformer plugin example dev.jag-k.gitlab` prints a copyable
starting point; `plugin doctor dev.jag-k.gitlab` explains why titles are
disabled when something is missing.
