use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use ct_config::{ConfigDocument, GroupDescriptor, GroupStatus};
use ct_core::RawRule;

use crate::state::GroupState;

/// Conservative safety limits for group membership.
///
/// These values are not configuration knobs; they guard against accidental
/// misuse and can be tuned after measurement.
pub const MAX_GROUP_ID_LENGTH: usize = 128;
pub const MAX_DISTINCT_GROUPS: usize = 1024;
pub const MAX_RULE_GROUPS: usize = 32;
pub const MAX_VISIBLE_TRAY_GROUPS: usize = 64;

/// Membership and descriptors for rule groups, resolved from a loaded config.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupPolicy {
    /// Effective group descriptors after merging root declarations and imports.
    pub descriptors: BTreeMap<String, GroupDescriptor>,
    /// For every rule id, the effective (inherited and deduplicated) group ids.
    pub rule_groups: BTreeMap<String, BTreeSet<String>>,
}

impl GroupPolicy {
    /// Build a policy from a loaded document.
    pub fn from_document(document: &ConfigDocument) -> Self {
        let descriptors = document.groups.clone();
        let mut rule_groups = BTreeMap::new();
        collect_rule_groups(&document.rules, &BTreeSet::new(), &mut rule_groups);

        Self {
            descriptors,
            rule_groups,
        }
    }

    /// Compute the set of rule IDs that should be disabled for a given state.
    pub fn disabled_rule_ids(&self, state: &GroupState) -> BTreeSet<String> {
        let mut disabled = BTreeSet::new();
        for (rule_id, groups) in &self.rule_groups {
            for group_id in groups {
                if self.is_ignored(group_id) {
                    continue;
                }
                if !state.is_enabled(group_id) {
                    disabled.insert(rule_id.clone());
                    break;
                }
            }
        }
        disabled
    }

    /// Return every rule controlled by at least one non-ignored group. Used as
    /// a fail-closed fallback when no valid state document has been read yet.
    pub fn controlled_rule_ids(&self) -> BTreeSet<String> {
        self.rule_groups
            .iter()
            .filter(|(_, groups)| groups.iter().any(|id| !self.is_ignored(id)))
            .map(|(rule_id, _)| rule_id.clone())
            .collect()
    }

    /// Return visible groups, limited to a reasonable tray size.
    pub fn visible_groups(&self, limit: usize) -> (Vec<(String, GroupDescriptor)>, usize) {
        let mut visible: Vec<_> = self
            .descriptors
            .iter()
            .filter(|(id, descriptor)| {
                descriptor.status == GroupStatus::Visible
                    && self
                        .rule_groups
                        .values()
                        .any(|groups| groups.contains(id.as_str()))
            })
            .map(|(id, descriptor)| (id.clone(), descriptor.clone()))
            .collect();
        let overflow = visible.len().saturating_sub(limit);
        visible.truncate(limit);
        (visible, overflow)
    }

    /// Groups whose descriptor status is `Ignore` are removed from evaluation.
    pub fn is_ignored(&self, group_id: &str) -> bool {
        self.descriptors
            .get(group_id)
            .map(|descriptor| descriptor.status == GroupStatus::Ignore)
            .unwrap_or(false)
    }

    /// Label to show for a group ID (name or id).
    pub fn group_label(&self, group_id: &str) -> String {
        self.descriptors
            .get(group_id)
            .and_then(|descriptor| descriptor.name.clone())
            .unwrap_or_else(|| group_id.to_string())
    }

    /// Validate that in-use group IDs do not exceed safety limits.
    pub fn validate(&self) -> Result<Vec<GroupValidationIssue>> {
        let mut issues = Vec::new();
        let all_ids: BTreeSet<_> = self
            .rule_groups
            .values()
            .flat_map(|groups| groups.iter())
            .chain(self.descriptors.keys())
            .cloned()
            .collect();

        if all_ids.len() > MAX_DISTINCT_GROUPS {
            issues.push(GroupValidationIssue::TooManyGroups {
                count: all_ids.len(),
                limit: MAX_DISTINCT_GROUPS,
            });
        }

        for id in &all_ids {
            if id.len() > MAX_GROUP_ID_LENGTH {
                issues.push(GroupValidationIssue::GroupIdTooLong {
                    id: id.clone(),
                    limit: MAX_GROUP_ID_LENGTH,
                });
            }
        }

        for (rule_id, groups) in &self.rule_groups {
            if groups.len() > MAX_RULE_GROUPS {
                issues.push(GroupValidationIssue::RuleGroupsTooMany {
                    rule_id: rule_id.clone(),
                    count: groups.len(),
                    limit: MAX_RULE_GROUPS,
                });
            }
        }

        Ok(issues)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupValidationIssue {
    TooManyGroups {
        count: usize,
        limit: usize,
    },
    GroupIdTooLong {
        id: String,
        limit: usize,
    },
    RuleGroupsTooMany {
        rule_id: String,
        count: usize,
        limit: usize,
    },
}

impl std::fmt::Display for GroupValidationIssue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyGroups { count, limit } => {
                write!(formatter, "too many distinct group ids ({count} > {limit})")
            }
            Self::GroupIdTooLong { id, limit } => {
                write!(formatter, "group id {id:?} exceeds maximum length {limit}")
            }
            Self::RuleGroupsTooMany {
                rule_id,
                count,
                limit,
            } => write!(
                formatter,
                "rule {rule_id:?} has too many groups ({count} > {limit})"
            ),
        }
    }
}

fn collect_rule_groups(
    rules: &[RawRule],
    inherited: &BTreeSet<String>,
    rule_groups: &mut BTreeMap<String, BTreeSet<String>>,
) {
    for rule in rules {
        let own: BTreeSet<_> = rule.groups.iter().cloned().collect();
        let mut effective = inherited.clone();
        effective.extend(own);

        if !rule.id.is_empty() {
            rule_groups.insert(rule.id.clone(), effective.clone());
        }

        if !rule.rules.is_empty() {
            collect_rule_groups(&rule.rules, &effective, rule_groups);
        }
    }
}

/// Resolve the group state file path from inputs.
pub fn resolve_group_state_path(
    explicit: Option<&Path>,
    state_dir: Option<&Path>,
) -> Result<Option<PathBuf>> {
    if let Some(path) = explicit {
        return Ok(Some(path.to_path_buf()));
    }
    Ok(state_dir.map(|dir| dir.join(GroupState::FILE_NAME)))
}

/// Load group state from an optional path. Missing files default to enabled.
pub fn load_group_state(path: Option<&Path>) -> Result<GroupState> {
    match path {
        Some(path) => GroupState::load(path).with_context(|| {
            format!(
                "load group state from {}; repair it, delete it to reset all groups, or use --ignore-group-state",
                path.display()
            )
        }),
        None => Ok(GroupState::default()),
    }
}

/// Atomically write group state to a file.
pub fn save_group_state(state: &GroupState, path: &Path) -> Result<()> {
    state
        .save(path)
        .with_context(|| format!("save group state to {}", path.display()))
}

/// Result of one serialized group-state mutation.
#[derive(Debug)]
pub struct GroupStateUpdate {
    pub state: GroupState,
    /// Parse/read error that caused an app-owned state file to be replaced
    /// from the caller's last-known-good snapshot (or a fresh state).
    pub recovered_from: Option<String>,
}

/// Update one group against the latest state while holding a stable sibling
/// lock. Locking the destination itself would not survive atomic replacement.
/// If the app-owned document is unreadable, an explicit user mutation repairs
/// it from `fallback`, or from a fresh state when no snapshot exists.
pub fn update_group_state(
    path: &Path,
    group_id: &str,
    enabled: bool,
    fallback: Option<&GroupState>,
) -> Result<GroupStateUpdate> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create group state directory {}", parent.display()))?;
    }
    let lock_path = path.with_extension("lock");
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open group state lock {}", lock_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("protect group state lock {}", lock_path.display()))?;
    }
    let mut lock = fd_lock::RwLock::new(lock_file);
    let _guard = lock
        .write()
        .with_context(|| format!("lock group state {}", lock_path.display()))?;
    let (mut state, recovered_from) = match load_group_state(Some(path)) {
        Ok(state) => (state, None),
        Err(error) => (
            fallback.cloned().unwrap_or_else(|| GroupState {
                version: GroupState::VERSION,
                ..GroupState::default()
            }),
            Some(format!("{error:#}")),
        ),
    };
    state.set_enabled(group_id, enabled);
    save_group_state(&state, path)?;
    Ok(GroupStateUpdate {
        state,
        recovered_from,
    })
}

/// Dedicated desktop watcher for the mutable group-state document. It watches
/// the parent so atomic replace and first-file creation work on every backend.
#[cfg(feature = "desktop")]
pub struct GroupStateWatcher {
    _watcher: notify::RecommendedWatcher,
}

#[cfg(feature = "desktop")]
impl GroupStateWatcher {
    pub fn new(path: PathBuf, on_change: std::sync::Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        use notify::{Config, RecursiveMode, Watcher};

        let parent = path
            .parent()
            .context("group state path has no parent directory")?
            .to_path_buf();
        std::fs::create_dir_all(&parent)
            .with_context(|| format!("create group state directory {}", parent.display()))?;
        let watched_path = path.clone();
        let mut watcher = notify::RecommendedWatcher::new(
            move |event| {
                if group_state_event_is_relevant(&event, &watched_path) {
                    on_change();
                }
            },
            Config::default(),
        )?;
        watcher.watch(&parent, RecursiveMode::NonRecursive)?;
        Ok(Self { _watcher: watcher })
    }
}

#[cfg(feature = "desktop")]
fn group_state_event_is_relevant(event: &notify::Result<notify::Event>, path: &Path) -> bool {
    let Ok(event) = event else {
        return true;
    };
    if event.paths.is_empty() {
        return true;
    }
    let temporary = path.with_extension("tmp");
    event
        .paths
        .iter()
        .any(|event_path| event_path == path || event_path == &temporary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_group_state_updates_preserve_both_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(GroupState::FILE_NAME);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let workers = ["alpha", "beta"].map(|id| {
            let path = path.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                update_group_state(&path, id, false, None).unwrap();
            })
        });
        barrier.wait();
        for worker in workers {
            worker.join().unwrap();
        }

        let state = GroupState::load(&path).unwrap();
        assert!(!state.is_enabled("alpha"));
        assert!(!state.is_enabled("beta"));
    }

    #[test]
    fn malformed_group_state_is_replaced_from_the_supplied_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(GroupState::FILE_NAME);
        std::fs::write(&path, b"not json").unwrap();
        let mut fallback = GroupState {
            version: GroupState::VERSION,
            ..GroupState::default()
        };
        fallback.set_enabled("existing", false);

        let update = update_group_state(&path, "privacy", false, Some(&fallback)).unwrap();
        assert!(update.recovered_from.is_some());
        assert!(!update.state.is_enabled("existing"));
        assert!(!update.state.is_enabled("privacy"));
        assert_eq!(GroupState::load(&path).unwrap(), update.state);
    }

    #[test]
    fn malformed_group_state_without_a_snapshot_is_replaced_from_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(GroupState::FILE_NAME);
        std::fs::write(&path, b"not json").unwrap();

        let update = update_group_state(&path, "privacy", false, None).unwrap();
        assert!(update.recovered_from.is_some());
        assert!(!update.state.is_enabled("privacy"));
        assert_eq!(GroupState::load(&path).unwrap(), update.state);
    }

    #[test]
    fn visible_group_limit_reports_overflow_without_changing_policy() {
        let mut policy = GroupPolicy::default();
        for index in 0..3 {
            let id = format!("group-{index}");
            policy.descriptors.insert(
                id.clone(),
                GroupDescriptor {
                    status: GroupStatus::Visible,
                    ..GroupDescriptor::default()
                },
            );
            policy
                .rule_groups
                .insert(format!("rule-{index}"), [id].into_iter().collect());
        }

        let (visible, overflow) = policy.visible_groups(2);
        assert_eq!(visible.len(), 2);
        assert_eq!(overflow, 1);
        assert_eq!(policy.controlled_rule_ids().len(), 3);
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn group_state_event_filter_accepts_atomic_target_and_temporary_paths() {
        let path = PathBuf::from("/state/groups.json");
        let target = notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(path.clone());
        let temporary =
            notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
                .add_path(path.with_extension("tmp"));
        let unrelated =
            notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Any))
                .add_path(PathBuf::from("/state/history.cbor"));

        assert!(group_state_event_is_relevant(&Ok(target), &path));
        assert!(group_state_event_is_relevant(&Ok(temporary), &path));
        assert!(!group_state_event_is_relevant(&Ok(unrelated), &path));
    }
}
