use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use ct_clipboard::{ClipboardFingerprint, ClipboardItem};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastClipboardSnapshot {
    pub version: u32,
    pub item: ClipboardItem,
    pub observed_at_unix_ms: u64,
    pub change_count: Option<u64>,
}

impl LastClipboardSnapshot {
    pub const VERSION: u32 = 1;

    pub fn load(path: &Path) -> Result<Option<Self>> {
        match fs::File::open(path) {
            Ok(file) => {
                let value: Self = ciborium::from_reader(file)
                    .with_context(|| format!("parse {}", path.display()))?;
                if value.version != Self::VERSION {
                    anyhow::bail!(
                        "unsupported last clipboard version {} in {}",
                        value.version,
                        path.display()
                    );
                }
                Ok(Some(value))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        Self::save_item(
            path,
            &self.item,
            self.observed_at_unix_ms,
            self.change_count,
        )
    }

    pub fn save_item(
        path: &Path,
        item: &ClipboardItem,
        observed_at_unix_ms: u64,
        change_count: Option<u64>,
    ) -> Result<()> {
        #[derive(Serialize)]
        struct BorrowedSnapshot<'a> {
            version: u32,
            item: &'a ClipboardItem,
            observed_at_unix_ms: u64,
            change_count: Option<u64>,
        }

        prepare_parent(path)?;
        let temporary = path.with_extension("tmp");
        let mut file = fs::File::create(&temporary)
            .with_context(|| format!("write {}", temporary.display()))?;
        protect_file(&temporary)?;
        ciborium::into_writer(
            &BorrowedSnapshot {
                version: Self::VERSION,
                item,
                observed_at_unix_ms,
                change_count,
            },
            &mut file,
        )
        .context("encode last clipboard as CBOR")?;
        file.flush()
            .with_context(|| format!("flush {}", temporary.display()))?;
        drop(file);
        replace_file(&temporary, path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastTransform {
    pub transform_id: Uuid,
    pub rule_id: Option<String>,
    pub previous: ClipboardItem,
    pub transformed: ClipboardItem,
    pub notification_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryRule {
    pub id: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryRecord {
    pub transform: LastTransform,
    pub transformed_at_unix_ms: u64,
    pub rules: Vec<HistoryRule>,
}

impl HistoryRecord {
    pub fn size_bytes(&self) -> usize {
        self.transform
            .previous
            .size_bytes()
            .saturating_add(self.transform.transformed.size_bytes())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistentAppState {
    pub version: u32,
    pub paused: bool,
    /// Absolute Unix timestamps; unlike Instant, these survive restarts.
    pub disabled_rules_until_unix_ms: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistentHistory {
    pub version: u32,
    pub items: Vec<HistoryRecord>,
}

macro_rules! persistent_json {
    ($type:ty) => {
        impl $type {
            pub const VERSION: u32 = 1;

            pub fn load(path: &Path) -> Result<Self> {
                match fs::read(path) {
                    Ok(bytes) => {
                        let value: Self = serde_json::from_slice(&bytes)
                            .with_context(|| format!("parse {}", path.display()))?;
                        if value.version != Self::VERSION {
                            anyhow::bail!(
                                "unsupported state version {} in {}",
                                value.version,
                                path.display()
                            );
                        }
                        Ok(value)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                        version: Self::VERSION,
                        ..Self::default()
                    }),
                    Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
                }
            }

            pub fn save(&self, path: &Path) -> Result<()> {
                save_compact_json(path, self)
            }
        }
    };
}

persistent_json!(PersistentAppState);

fn enabled() -> bool {
    true
}

fn is_enabled(value: &bool) -> bool {
    *value
}

/// One entry in the group state document. Missing means the group is enabled.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupStateEntry {
    #[serde(default = "enabled", skip_serializing_if = "is_enabled")]
    pub enabled: bool,
}

/// Persistent enable/disable state for rule groups.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupState {
    pub version: u32,
    pub groups: BTreeMap<String, GroupStateEntry>,
}

impl GroupState {
    pub const VERSION: u32 = 1;
    pub const FILE_NAME: &str = "groups.json";

    pub fn load(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                let value: Self = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parse {}", path.display()))?;
                if value.version != Self::VERSION {
                    anyhow::bail!(
                        "unsupported group state version {} in {}",
                        value.version,
                        path.display()
                    );
                }
                Ok(value)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                version: Self::VERSION,
                ..Self::default()
            }),
            Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        save_compact_json(path, self)
    }

    pub fn is_enabled(&self, group_id: &str) -> bool {
        self.groups
            .get(group_id)
            .map(|entry| entry.enabled)
            .unwrap_or(true)
    }

    pub fn set_enabled(&mut self, group_id: &str, enabled: bool) {
        if enabled {
            if let Some(entry) = self.groups.get_mut(group_id) {
                if entry.enabled {
                    return;
                }
                entry.enabled = true;
            }
            // No entry means enabled; no need to create one.
        } else {
            self.groups
                .insert(group_id.to_string(), GroupStateEntry { enabled: false });
        }
    }

    pub fn list(&self) -> impl Iterator<Item = (&str, bool)> {
        self.groups
            .iter()
            .map(|(id, entry)| (id.as_str(), entry.enabled))
    }
}

impl PersistentHistory {
    pub const VERSION: u32 = 1;

    pub fn load(path: &Path) -> Result<Self> {
        match fs::File::open(path) {
            Ok(file) => {
                let value: Self = ciborium::from_reader(file)
                    .with_context(|| format!("parse {}", path.display()))?;
                if value.version != Self::VERSION {
                    anyhow::bail!(
                        "unsupported history version {} in {}",
                        value.version,
                        path.display()
                    );
                }
                Ok(value)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                version: Self::VERSION,
                ..Self::default()
            }),
            Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        prepare_parent(path)?;
        let temporary = path.with_extension("tmp");
        let mut file = fs::File::create(&temporary)
            .with_context(|| format!("write {}", temporary.display()))?;
        protect_file(&temporary)?;
        ciborium::into_writer(self, &mut file).context("encode history as CBOR")?;
        file.flush()
            .with_context(|| format!("flush {}", temporary.display()))?;
        drop(file);
        replace_file(&temporary, path)
    }

    fn append(&mut self, record: HistoryRecord) {
        self.items.insert(0, record);
    }

    pub(crate) fn prune(&mut self, max_items: usize, max_bytes: u64) {
        self.items.truncate(max_items);
        if max_bytes == 0 {
            return;
        }

        let mut total = self.items.iter().fold(0u128, |total, record| {
            total.saturating_add(record.size_bytes() as u128)
        });
        while total > u128::from(max_bytes) {
            let Some(removed) = self.items.pop() else {
                break;
            };
            total = total.saturating_sub(removed.size_bytes() as u128);
        }
    }
}

enum HistoryCommand {
    AppendAndPrune {
        record: Box<HistoryRecord>,
        max_items: usize,
        max_bytes: u64,
    },
    Prune {
        max_items: usize,
        max_bytes: u64,
    },
    Clear,
    Flush(mpsc::SyncSender<()>),
    Shutdown(mpsc::SyncSender<()>),
}

pub struct HistoryWriter {
    sender: Option<mpsc::Sender<HistoryCommand>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl HistoryWriter {
    pub fn start(path: PathBuf, history: PersistentHistory) -> Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("history-writer".into())
            .spawn(move || history_writer_loop(path, history, receiver))
            .context("spawn history writer")?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
        })
    }

    pub fn append_and_prune(&self, record: HistoryRecord, max_items: usize, max_bytes: u64) {
        self.send(HistoryCommand::AppendAndPrune {
            record: Box::new(record),
            max_items,
            max_bytes,
        });
    }

    pub fn prune(&self, max_items: usize, max_bytes: u64) {
        self.send(HistoryCommand::Prune {
            max_items,
            max_bytes,
        });
    }

    pub fn clear(&self) {
        self.send(HistoryCommand::Clear);
    }

    pub fn flush(&self) {
        let (done_sender, done_receiver) = mpsc::sync_channel(0);
        if self
            .sender
            .as_ref()
            .is_none_or(|sender| sender.send(HistoryCommand::Flush(done_sender)).is_err())
        {
            log_history_error("history writer flush failed: worker unavailable");
            return;
        }
        if done_receiver.recv().is_err() {
            log_history_error("history writer flush failed: worker stopped");
        }
    }

    fn send(&self, command: HistoryCommand) {
        if self
            .sender
            .as_ref()
            .is_none_or(|sender| sender.send(command).is_err())
        {
            log_history_error("history writer command failed: worker unavailable");
        }
    }
}

impl Drop for HistoryWriter {
    fn drop(&mut self) {
        let Some(sender) = self.sender.take() else {
            return;
        };
        let (done_sender, done_receiver) = mpsc::sync_channel(0);
        if sender.send(HistoryCommand::Shutdown(done_sender)).is_ok() {
            let _ = done_receiver.recv();
        } else {
            log_history_error("history writer shutdown failed: worker unavailable");
        }
        if self
            .worker
            .take()
            .is_some_and(|worker| worker.join().is_err())
        {
            log_history_error("history writer shutdown failed: worker panicked");
        }
    }
}

fn history_writer_loop(
    path: PathBuf,
    mut history: PersistentHistory,
    receiver: mpsc::Receiver<HistoryCommand>,
) {
    while let Ok(command) = receiver.recv() {
        match command {
            HistoryCommand::AppendAndPrune {
                record,
                max_items,
                max_bytes,
            } => {
                history.append(*record);
                history.prune(max_items, max_bytes);
                save_history_best_effort(&history, &path);
            }
            HistoryCommand::Prune {
                max_items,
                max_bytes,
            } => {
                history.prune(max_items, max_bytes);
                save_history_best_effort(&history, &path);
            }
            HistoryCommand::Clear => {
                history.items.clear();
                save_history_best_effort(&history, &path);
            }
            HistoryCommand::Flush(done) => {
                save_history_best_effort(&history, &path);
                let _ = done.send(());
            }
            HistoryCommand::Shutdown(done) => {
                save_history_best_effort(&history, &path);
                let _ = done.send(());
                break;
            }
        }
    }
}

fn save_history_best_effort(history: &PersistentHistory, path: &Path) {
    if let Err(error) = history.save(path) {
        log_history_error(format!("history save failed: {error:#}"));
    }
}

fn log_history_error(message: impl AsRef<str>) {
    crate::logging::event(message);
}

fn save_compact_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    save_bytes(path, &bytes)
}

fn save_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    prepare_parent(path)?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes).with_context(|| format!("write {}", temporary.display()))?;
    protect_file(&temporary)?;
    replace_file(&temporary, path)
}

fn prepare_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    Ok(())
}

fn protect_file(path: &Path) -> Result<()> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("protect {}", path.display()))?;
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn replace_file(temporary: &Path, destination: &Path) -> Result<()> {
    if let Err(first_error) = fs::rename(temporary, destination) {
        if destination.exists() {
            fs::remove_file(destination)
                .with_context(|| format!("replace {}", destination.display()))?;
            fs::rename(temporary, destination)
                .with_context(|| format!("replace {}", destination.display()))?;
        } else {
            return Err(first_error)
                .with_context(|| format!("move state to {}", destination.display()));
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn replace_file(temporary: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    use windows_sys::Win32::Foundation::{GetLastError, ERROR_UNABLE_TO_MOVE_REPLACEMENT_2};
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};

    if !destination.exists() {
        match fs::rename(temporary, destination) {
            Ok(()) => return Ok(()),
            Err(first_error) if !destination.exists() => {
                return Err(first_error)
                    .with_context(|| format!("move state to {}", destination.display()));
            }
            Err(_) => {
                // The destination appeared after the existence check; replace it below.
            }
        }
    }

    let backup = destination.with_file_name(format!(
        ".{}.replace-backup-{}",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("state"),
        Uuid::new_v4()
    ));
    let destination_wide: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let temporary_wide: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let backup_wide: Vec<u16> = backup.as_os_str().encode_wide().chain(Some(0)).collect();

    // SAFETY: All three paths are valid, null-terminated UTF-16 strings and the
    // reserved pointer parameters are null as required by ReplaceFileW.
    let replaced = unsafe {
        ReplaceFileW(
            destination_wide.as_ptr(),
            temporary_wide.as_ptr(),
            backup_wide.as_ptr(),
            REPLACEFILE_WRITE_THROUGH,
            ptr::null(),
            ptr::null(),
        )
    };
    if replaced != 0 {
        let _ = fs::remove_file(backup);
        return Ok(());
    }

    // SAFETY: GetLastError has no preconditions and is called immediately after
    // ReplaceFileW failed on this thread.
    let error_code = unsafe { GetLastError() };
    let replace_error = std::io::Error::from_raw_os_error(error_code as i32);
    if error_code == ERROR_UNABLE_TO_MOVE_REPLACEMENT_2 {
        fs::rename(&backup, destination).with_context(|| {
            format!(
                "restore {} from {} after replacement failed: {replace_error}",
                destination.display(),
                backup.display()
            )
        })?;
    }

    Err(replace_error).with_context(|| format!("replace {}", destination.display()))
}

pub fn quarantine_corrupt_file(path: &Path) -> Result<Option<std::path::PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    let quarantine = path.with_file_name(format!("{file_name}.corrupt-{timestamp}"));
    fs::rename(path, &quarantine).with_context(|| {
        format!(
            "quarantine corrupt state {} as {}",
            path.display(),
            quarantine.display()
        )
    })?;
    Ok(Some(quarantine))
}

pub fn remove_corrupt_file(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => {
            Err(error).with_context(|| format!("remove corrupt state {}", path.display()))
        }
    }
}

#[derive(Debug, Default)]
pub struct UndoState {
    entries: VecDeque<LastTransform>,
}

impl UndoState {
    pub fn remember(&mut self, transform: LastTransform) {
        self.entries
            .retain(|entry| entry.transform_id != transform.transform_id);
        self.entries.push_front(transform);
    }

    pub fn clear(&mut self) -> Vec<String> {
        self.entries
            .drain(..)
            .map(|entry| entry.notification_id)
            .collect()
    }

    pub fn truncate(&mut self, len: usize) -> Vec<String> {
        if len >= self.entries.len() {
            return Vec::new();
        }
        self.entries
            .split_off(len)
            .into_iter()
            .map(|entry| entry.notification_id)
            .collect()
    }

    pub fn retain_ids(&mut self, ids: &std::collections::BTreeSet<Uuid>) -> Vec<String> {
        let mut removed = Vec::new();
        self.entries.retain(|entry| {
            if ids.contains(&entry.transform_id) {
                true
            } else {
                removed.push(entry.notification_id.clone());
                false
            }
        });
        removed
    }

    pub fn undo(&mut self, transform_id: Uuid, current: &ClipboardItem) -> Option<ClipboardItem> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.transform_id == transform_id)?;
        if !clipboard_payload_matches(&entry.transformed, current) {
            return None;
        }
        Some(entry.previous.clone())
    }

    pub fn restore(&self, transform_id: Uuid) -> Option<ClipboardItem> {
        self.entries
            .iter()
            .find(|entry| entry.transform_id == transform_id)
            .map(|entry| entry.previous.clone())
    }

    pub fn contains(&self, transform_id: Uuid) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.transform_id == transform_id)
    }

    pub fn latest_notification_id(&self) -> Option<&str> {
        self.entries
            .front()
            .map(|entry| entry.notification_id.as_str())
    }

    pub fn notification_ids(&self) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .map(|entry| entry.notification_id.as_str())
    }

    pub fn notification_id(&self, transform_id: Uuid) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.transform_id == transform_id)
            .map(|entry| entry.notification_id.as_str())
    }
}

#[derive(Debug, Default)]
pub struct ClipboardWriteGuard {
    expected_own_write: Option<ClipboardFingerprint>,
}

impl ClipboardWriteGuard {
    pub fn mark_own_write(&mut self, content: &ClipboardItem) {
        self.expected_own_write = Some(content.fingerprint());
    }

    pub fn classify_change(&mut self, current: &ClipboardItem) -> ClipboardChangeKind {
        if self
            .expected_own_write
            .as_ref()
            .is_some_and(|expected| expected.matches_own_write(current.fingerprint()))
        {
            self.expected_own_write = None;
            ClipboardChangeKind::OwnWrite
        } else {
            self.expected_own_write = None;
            ClipboardChangeKind::External
        }
    }
}

fn clipboard_payload_matches(expected: &ClipboardItem, current: &ClipboardItem) -> bool {
    expected.payload_eq(current)
        || expected
            .text()
            .zip(current.text())
            .is_some_and(|(expected, current)| expected == current)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardChangeKind {
    OwnWrite,
    External,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_clipboard::{ClipboardFormat, ClipboardSourceApp};

    fn history_record(before: &str, after: &str) -> HistoryRecord {
        HistoryRecord {
            transform: LastTransform {
                transform_id: Uuid::new_v4(),
                rule_id: Some("rule".into()),
                previous: ClipboardItem::from_text(before),
                transformed: ClipboardItem::from_text(after),
                notification_id: "notification".into(),
            },
            transformed_at_unix_ms: 0,
            rules: Vec::new(),
        }
    }

    #[test]
    fn undo_rejects_stale_clipboard() {
        let mut state = UndoState::default();
        let transform_id = Uuid::new_v4();
        state.remember(LastTransform {
            transform_id,
            rule_id: Some("rule".into()),
            previous: ClipboardItem::from_text("before"),
            transformed: ClipboardItem::from_text("after"),
            notification_id: "notification".into(),
        });

        assert!(state
            .undo(transform_id, &ClipboardItem::from_text("something else"))
            .is_none());
    }

    #[test]
    fn undo_accepts_latest_matching_transform() {
        let mut state = UndoState::default();
        let transform_id = Uuid::new_v4();
        state.remember(LastTransform {
            transform_id,
            rule_id: Some("rule".into()),
            previous: ClipboardItem::from_text("before"),
            transformed: ClipboardItem::from_text("after"),
            notification_id: "notification".into(),
        });

        assert_eq!(
            state.undo(transform_id, &ClipboardItem::from_text("after")),
            Some(ClipboardItem::from_text("before"))
        );
    }

    #[test]
    fn restore_returns_original_without_matching_current_clipboard() {
        let mut state = UndoState::default();
        let transform_id = Uuid::new_v4();
        state.remember(LastTransform {
            transform_id,
            rule_id: Some("rule".into()),
            previous: ClipboardItem::from_text("before"),
            transformed: ClipboardItem::from_text("after"),
            notification_id: "notification".into(),
        });

        assert_eq!(
            state.restore(transform_id),
            Some(ClipboardItem::from_text("before"))
        );
        assert!(state.contains(transform_id));
    }

    #[test]
    fn undo_can_restore_any_matching_history_entry() {
        let mut state = UndoState::default();
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        state.remember(LastTransform {
            transform_id: first_id,
            rule_id: Some("first".into()),
            previous: ClipboardItem::from_text("one"),
            transformed: ClipboardItem::from_text("two"),
            notification_id: "first".into(),
        });
        state.remember(LastTransform {
            transform_id: second_id,
            rule_id: Some("second".into()),
            previous: ClipboardItem::from_text("three"),
            transformed: ClipboardItem::from_text("four"),
            notification_id: "second".into(),
        });

        assert!(state.contains(first_id));
        assert_eq!(
            state.undo(first_id, &ClipboardItem::from_text("two")),
            Some(ClipboardItem::from_text("one"))
        );
        assert!(state.contains(second_id));
    }

    #[test]
    fn own_write_matches_when_macos_adds_url_format() {
        let mut guard = ClipboardWriteGuard::default();
        guard.mark_own_write(&ClipboardItem::from_text("https://example.com/page"));

        let mut current = ClipboardItem::from_text("https://example.com/page");
        current.set(
            ClipboardFormat::new("public.url"),
            "https://example.com/page".to_string(),
        );

        assert_eq!(
            guard.classify_change(&current),
            ClipboardChangeKind::OwnWrite
        );
    }

    #[test]
    fn payload_matching_ignores_source_application_metadata() {
        let expected = ClipboardItem::from_text("same").with_source_app(ClipboardSourceApp::new(
            Some("source.app".into()),
            Some("Source".into()),
        ));
        let current = ClipboardItem::from_text("same").with_source_app(ClipboardSourceApp::new(
            Some("target.app".into()),
            Some("Target".into()),
        ));

        assert!(clipboard_payload_matches(&expected, &current));
    }

    #[test]
    fn undo_matches_when_macos_adds_url_format() {
        let mut state = UndoState::default();
        let transform_id = Uuid::new_v4();
        state.remember(LastTransform {
            transform_id,
            rule_id: Some("rule".into()),
            previous: ClipboardItem::from_text("before"),
            transformed: ClipboardItem::from_text("https://example.com/page"),
            notification_id: "notification".into(),
        });

        let mut current = ClipboardItem::from_text("https://example.com/page");
        current.set(
            ClipboardFormat::new("public.url"),
            "https://example.com/page".to_string(),
        );

        assert_eq!(
            state.undo(transform_id, &current),
            Some(ClipboardItem::from_text("before"))
        );
    }

    #[test]
    fn persistent_state_and_history_round_trip_compactly() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.json");
        let history_path = dir.path().join("history.cbor");
        let mut previous = ClipboardItem::from_text("before");
        previous.set_bytes(ClipboardFormat::new("public.png"), vec![0, 1, 2, 255]);
        let history = PersistentHistory {
            version: PersistentHistory::VERSION,
            items: vec![HistoryRecord {
                transform: LastTransform {
                    transform_id: Uuid::new_v4(),
                    rule_id: Some("rule".into()),
                    previous,
                    transformed: ClipboardItem::from_text("after"),
                    notification_id: "notification".into(),
                },
                transformed_at_unix_ms: 1234,
                rules: vec![HistoryRule {
                    id: "rule".into(),
                    name: Some("Rule".into()),
                }],
            }],
        };
        let state = PersistentAppState {
            version: PersistentAppState::VERSION,
            paused: true,
            disabled_rules_until_unix_ms: [("rule".into(), 5678)].into(),
        };

        state.save(&state_path).unwrap();
        history.save(&history_path).unwrap();

        assert_eq!(PersistentAppState::load(&state_path).unwrap(), state);
        assert_eq!(PersistentHistory::load(&history_path).unwrap(), history);
        assert_ne!(fs::read(&history_path).unwrap().first(), Some(&b'{'));
    }

    #[test]
    fn last_clipboard_snapshot_round_trips_as_cbor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("last-clipboard.cbor");
        let snapshot = LastClipboardSnapshot {
            version: LastClipboardSnapshot::VERSION,
            item: ClipboardItem::from_text("copied"),
            observed_at_unix_ms: 1234,
            change_count: Some(42),
        };

        snapshot.save(&path).unwrap();

        assert_eq!(LastClipboardSnapshot::load(&path).unwrap(), Some(snapshot));
        assert_ne!(fs::read(&path).unwrap().first(), Some(&b'{'));
    }

    #[test]
    fn pre_native_descriptor_cbor_is_quarantined_and_replaced() {
        #[derive(Serialize)]
        struct LegacyItem {
            representations: BTreeMap<String, serde_bytes::ByteBuf>,
            source_app: Option<ct_clipboard::ClipboardSourceApp>,
        }
        #[derive(Serialize)]
        struct LegacySnapshot {
            version: u32,
            item: LegacyItem,
            observed_at_unix_ms: u64,
            change_count: Option<u64>,
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("last-clipboard.cbor");
        let file = fs::File::create(&path).unwrap();
        ciborium::into_writer(
            &LegacySnapshot {
                version: 1,
                item: LegacyItem {
                    representations: [(
                        "public.utf8-plain-text".into(),
                        serde_bytes::ByteBuf::from(b"legacy".to_vec()),
                    )]
                    .into(),
                    source_app: None,
                },
                observed_at_unix_ms: 1,
                change_count: Some(2),
            },
            file,
        )
        .unwrap();

        assert!(LastClipboardSnapshot::load(&path).is_err());
        let quarantined = quarantine_corrupt_file(&path).unwrap().unwrap();
        assert!(quarantined.is_file());
        assert!(!path.exists());

        let replacement = LastClipboardSnapshot {
            version: LastClipboardSnapshot::VERSION,
            item: ClipboardItem::from_text("new"),
            observed_at_unix_ms: 3,
            change_count: Some(4),
        };
        replacement.save(&path).unwrap();
        assert_eq!(
            LastClipboardSnapshot::load(&path).unwrap(),
            Some(replacement)
        );
    }

    #[test]
    fn pre_native_descriptor_history_does_not_block_empty_fallback() {
        #[derive(Clone, Serialize)]
        struct LegacyItem {
            representations: BTreeMap<String, serde_bytes::ByteBuf>,
            source_app: Option<ct_clipboard::ClipboardSourceApp>,
        }
        #[derive(Serialize)]
        struct LegacyTransform {
            transform_id: Uuid,
            rule_id: Option<String>,
            previous: LegacyItem,
            transformed: LegacyItem,
            notification_id: String,
        }
        #[derive(Serialize)]
        struct LegacyRecord {
            transform: LegacyTransform,
            transformed_at_unix_ms: u64,
            rules: Vec<HistoryRule>,
        }
        #[derive(Serialize)]
        struct LegacyHistory {
            version: u32,
            items: Vec<LegacyRecord>,
        }

        let legacy_item = LegacyItem {
            representations: [(
                "public.utf8-plain-text".into(),
                serde_bytes::ByteBuf::from(b"legacy".to_vec()),
            )]
            .into(),
            source_app: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.cbor");
        let file = fs::File::create(&path).unwrap();
        ciborium::into_writer(
            &LegacyHistory {
                version: 1,
                items: vec![LegacyRecord {
                    transform: LegacyTransform {
                        transform_id: Uuid::new_v4(),
                        rule_id: Some("legacy".into()),
                        previous: legacy_item.clone(),
                        transformed: legacy_item,
                        notification_id: "legacy".into(),
                    },
                    transformed_at_unix_ms: 1,
                    rules: Vec::new(),
                }],
            },
            file,
        )
        .unwrap();

        assert!(PersistentHistory::load(&path).is_err());
        assert!(quarantine_corrupt_file(&path).unwrap().is_some());
        let empty = PersistentHistory {
            version: PersistentHistory::VERSION,
            items: Vec::new(),
        };
        empty.save(&path).unwrap();
        assert_eq!(PersistentHistory::load(&path).unwrap(), empty);
    }

    #[test]
    fn history_writer_applies_commands_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.cbor");
        let writer =
            HistoryWriter::start(path.clone(), PersistentHistory::load(&path).unwrap()).unwrap();
        let discarded = history_record("first", "discarded");
        let retained = history_record("second", "retained");
        let retained_id = retained.transform.transform_id;

        writer.append_and_prune(discarded, usize::MAX, 0);
        writer.clear();
        writer.append_and_prune(retained, usize::MAX, 0);
        writer.flush();

        let history = PersistentHistory::load(&path).unwrap();
        assert_eq!(history.items.len(), 1);
        assert_eq!(history.items[0].transform.transform_id, retained_id);
    }

    #[test]
    fn history_writer_prunes_count_and_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.cbor");
        let writer =
            HistoryWriter::start(path.clone(), PersistentHistory::load(&path).unwrap()).unwrap();
        let oldest = history_record("old", "entry");
        let middle = history_record("cat", "dog");
        let newest = history_record("one", "two");
        let newest_id = newest.transform.transform_id;

        writer.append_and_prune(oldest, 2, 6);
        writer.append_and_prune(middle, 2, 6);
        writer.append_and_prune(newest, 2, 6);
        writer.flush();

        let history = PersistentHistory::load(&path).unwrap();
        assert_eq!(history.items.len(), 1);
        assert_eq!(history.items[0].transform.transform_id, newest_id);
    }

    #[test]
    fn history_writer_drop_flushes_pending_commands() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.cbor");
        let record = history_record("before", "after");
        let transform_id = record.transform.transform_id;
        let writer =
            HistoryWriter::start(path.clone(), PersistentHistory::load(&path).unwrap()).unwrap();

        writer.append_and_prune(record, usize::MAX, 0);
        drop(writer);

        assert_eq!(
            PersistentHistory::load(&path).unwrap().items[0]
                .transform
                .transform_id,
            transform_id
        );
    }

    #[test]
    fn history_writer_continues_loaded_history_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.cbor");
        let first = history_record("first", "saved");
        let first_id = first.transform.transform_id;
        {
            let writer =
                HistoryWriter::start(path.clone(), PersistentHistory::load(&path).unwrap())
                    .unwrap();
            writer.append_and_prune(first, usize::MAX, 0);
        }
        let second = history_record("second", "saved");
        let second_id = second.transform.transform_id;
        {
            let history = PersistentHistory::load(&path).unwrap();
            let writer = HistoryWriter::start(path.clone(), history).unwrap();
            writer.append_and_prune(second, usize::MAX, 0);
        }

        let history = PersistentHistory::load(&path).unwrap();
        assert_eq!(history.items.len(), 2);
        assert_eq!(history.items[0].transform.transform_id, second_id);
        assert_eq!(history.items[1].transform.transform_id, first_id);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn replace_file_overwrites_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let temporary = dir.path().join("state.tmp");
        let destination = dir.path().join("state.json");
        fs::write(&temporary, b"new").unwrap();
        fs::write(&destination, b"old").unwrap();

        replace_file(&temporary, &destination).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert!(!temporary.exists());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn replace_file_failure_preserves_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let missing_temporary = dir.path().join("missing.tmp");
        let destination = dir.path().join("state.json");
        fs::write(&destination, b"old").unwrap();

        assert!(replace_file(&missing_temporary, &destination).is_err());

        assert_eq!(fs::read(&destination).unwrap(), b"old");
    }
}
