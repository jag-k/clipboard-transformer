// Settings shapes shared by the plugin code and the manifest generator: the
// doc comments below become JSON Schema descriptions in the embedded
// manifest, so the config editor shows them.
//
// Compiled twice: into the wasm plugin (`mod settings;` in lib.rs) and into
// `build.rs` via `include!`. Keep it self-contained: serde only, no extism
// imports, no `//!` inner doc comments (include! rejects them); schemars
// derives are gated to non-wasm targets so the plugin binary never depends
// on schemars.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Plugin settings under `plugins.dev.jag-k.gitlab.settings`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[cfg_attr(not(target_arch = "wasm32"), derive(schemars::JsonSchema))]
#[serde(default)]
pub struct PluginSettings {
    /// GitLab instances the plugin may query for real resource titles and
    /// project names.
    /// Without instances the plugin still works offline and produces short
    /// labels like `org/team/project!123`.
    pub instances: Vec<Instance>,
}

/// One GitLab instance the plugin can talk to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(not(target_arch = "wasm32"), derive(schemars::JsonSchema))]
pub struct Instance {
    /// Host this instance serves, e.g. `gitlab.example.com`.
    pub host: String,
    /// API base URL. Defaults to `https://<host>/api/v4`.
    #[serde(default)]
    pub api_base: Option<String>,
    /// Personal access token sent as `PRIVATE-TOKEN`. Usually a
    /// `${GITLAB_TOKEN}` reference with the `env_expansion` permission
    /// granted. Omit it for instances with public projects only.
    #[serde(default)]
    pub token: Option<String>,
}

/// Per-rule settings shared by resource rule types that can query GitLab.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(not(target_arch = "wasm32"), derive(schemars::JsonSchema))]
#[serde(default)]
pub struct RuleSettings {
    /// GitLab hosts this rule matches. Defaults to the configured instance
    /// hosts, or `gitlab.com` when no instances are configured.
    pub hosts: Vec<String>,
    /// Fetch the real title when a configured instance and HTTP grant are
    /// available. Disable for deterministic offline-only labels.
    pub online: bool,
}

impl Default for RuleSettings {
    fn default() -> Self {
        Self {
            hosts: Vec::new(),
            online: true,
        }
    }
}

/// How a direct `#note_<id>` issue or merge request link is described.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(not(target_arch = "wasm32"), derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum CommentDisplay {
    /// Do not mention that the URL targets a comment.
    Hidden,
    /// Append `comment`, preserving the original plugin behavior.
    #[default]
    Marker,
    /// Append the GitLab note ID, e.g. `comment 123`.
    Id,
    /// Append `comment by @user`; falls back to the note ID when the author
    /// cannot be fetched.
    Author,
    /// Append both the note ID and author; falls back to the note ID when the
    /// author cannot be fetched.
    AuthorAndId,
}

/// Settings for the `mr` and `issue` rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(not(target_arch = "wasm32"), derive(schemars::JsonSchema))]
#[serde(default)]
pub struct DiscussionRuleSettings {
    /// GitLab hosts this rule matches. Defaults to the configured instance
    /// hosts, or `gitlab.com` when no instances are configured.
    pub hosts: Vec<String>,
    /// Fetch the resource title and, when requested, comment author from a
    /// configured instance. Disable for deterministic offline labels.
    pub online: bool,
    /// How direct comment links are identified in the Markdown label.
    pub comment_display: CommentDisplay,
}

impl Default for DiscussionRuleSettings {
    fn default() -> Self {
        Self {
            hosts: Vec::new(),
            online: true,
            comment_display: CommentDisplay::Marker,
        }
    }
}

/// Repository browser link kinds supported by the `repository` rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(not(target_arch = "wasm32"), derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum RepositoryLinkKind {
    /// A repository tree at a revision and optional directory.
    Tree,
    /// A rendered repository file.
    Blob,
    /// A raw repository file.
    Raw,
    /// A repository file blame view.
    Blame,
    /// A commit list for a revision.
    Commits,
    /// A comparison between revisions.
    Compare,
}

/// Settings for the `repository` rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(not(target_arch = "wasm32"), derive(schemars::JsonSchema))]
#[serde(default)]
pub struct RepositoryRuleSettings {
    /// GitLab hosts this rule matches. Defaults to the configured instance
    /// hosts, or `gitlab.com` when no instances are configured.
    pub hosts: Vec<String>,
    /// Fetch the project name when a configured instance and HTTP grant are
    /// available. Disable for deterministic offline labels.
    pub online: bool,
    /// Repository browser link kinds transformed by this rule.
    pub kinds: Vec<RepositoryLinkKind>,
}

impl Default for RepositoryRuleSettings {
    fn default() -> Self {
        Self {
            hosts: Vec::new(),
            online: true,
            kinds: vec![
                RepositoryLinkKind::Tree,
                RepositoryLinkKind::Blob,
                RepositoryLinkKind::Raw,
                RepositoryLinkKind::Blame,
                RepositoryLinkKind::Commits,
                RepositoryLinkKind::Compare,
            ],
        }
    }
}

/// Settings for the `project` rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(not(target_arch = "wasm32"), derive(schemars::JsonSchema))]
#[serde(default)]
pub struct ProjectRuleSettings {
    /// GitLab hosts this rule matches. Defaults to the configured instance
    /// hosts, or `gitlab.com` when no instances are configured.
    pub hosts: Vec<String>,
    /// Fetch the project name when a configured instance and HTTP grant are
    /// available. Disable for deterministic offline labels.
    pub online: bool,
    /// Exact paths below `/-/` and their display labels. Paths may contain
    /// multiple segments, for example `wikis/home`. The rule also always
    /// matches the bare project URL. Replace this map to select or add pages.
    pub aliases: BTreeMap<String, String>,
}

impl Default for ProjectRuleSettings {
    fn default() -> Self {
        Self {
            hosts: Vec::new(),
            online: true,
            aliases: BTreeMap::from([
                ("activity".to_string(), "Activity".to_string()),
                ("analytics".to_string(), "Analytics".to_string()),
                ("boards".to_string(), "Issue Boards".to_string()),
                ("branches".to_string(), "Branches".to_string()),
                ("ci/lint".to_string(), "CI Lint".to_string()),
                (
                    "container_registry".to_string(),
                    "Container Registry".to_string(),
                ),
                ("deployments".to_string(), "Deployments".to_string()),
                ("environments".to_string(), "Environments".to_string()),
                ("feature_flags".to_string(), "Feature Flags".to_string()),
                ("forks".to_string(), "Forks".to_string()),
                ("infrastructure".to_string(), "Infrastructure".to_string()),
                ("issues".to_string(), "Issues".to_string()),
                ("jobs".to_string(), "Jobs".to_string()),
                ("labels".to_string(), "Labels".to_string()),
                ("merge_requests".to_string(), "MRs".to_string()),
                ("milestones".to_string(), "Milestones".to_string()),
                ("packages".to_string(), "Packages".to_string()),
                (
                    "pipeline_schedules".to_string(),
                    "Pipeline Schedules".to_string(),
                ),
                ("pipelines".to_string(), "Pipelines".to_string()),
                ("project_members".to_string(), "Members".to_string()),
                ("releases".to_string(), "Releases".to_string()),
                (
                    "security/dashboard".to_string(),
                    "Security Dashboard".to_string(),
                ),
                ("snippets".to_string(), "Snippets".to_string()),
                ("tags".to_string(), "Tags".to_string()),
                ("wikis/home".to_string(), "Wiki".to_string()),
            ]),
        }
    }
}
