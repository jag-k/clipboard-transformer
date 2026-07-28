pub mod reload;
pub mod runtime;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use uuid::Uuid;

use crate::config::{ConfigDocument, ConfigWarning, EditorConfig, RuleSource};
use crate::logging;
use crate::platform::autostart::AutostartStatus;
use crate::platform::tray::{TrayRecentItem, TrayRule, TraySnapshot};
use crate::rules::{RuleWorker, RuleWorkerCompletion};
use crate::state::{
    quarantine_corrupt_file, remove_corrupt_file, ClipboardChangeKind, ClipboardWriteGuard,
    HistoryRecord, HistoryRule, HistoryWriter, LastClipboardSnapshot, LastTransform,
    PersistentAppState, PersistentHistory, UndoState,
};
use ct_clipboard::{ClipboardBackend, ClipboardFingerprint, ClipboardFormat, ClipboardItem};
use ct_core::{AppMatcher, AppliedRule, RuleEngine, TransformResult};
use ct_i18n::human_duration;
use ct_notifications::{
    EditTarget, NotificationBackend, StartupNotification, TransformNotification,
};

pub struct Agent<C, N> {
    clipboard: C,
    notifications: N,
    rule_worker: RuleWorker,
    next_rule_job_id: u64,
    active_rule_job: Option<PendingRuleJob>,
    queued_rule_job: Option<PreparedRuleJob>,
    rule_epoch: u64,
    required_formats: BTreeSet<ClipboardFormat>,
    config: ConfigDocument,
    app_matcher: AppMatcher,
    undo: UndoState,
    write_guard: ClipboardWriteGuard,
    disabled_rules: BTreeMap<String, SystemTime>,
    last_seen: Option<ClipboardFingerprint>,
    last_transformed_external: Option<RecentExternalCopy>,
    edit_config_path: Option<PathBuf>,
    rule_sources: BTreeMap<String, RuleSource>,
    recent_transforms: VecDeque<RecentTransform>,
    /// Where tray-visible state is published. `None` for hosts without a tray
    /// (the CLI, tests), which then simply have nobody reading it.
    tray_state: Option<crate::platform::tray::TrayStateHandle>,
    /// Counts snapshot builds published to the tray. Exists so tests can tell
    /// "nothing changed" from "rebuilt anyway": the second is the waste that a
    /// per-tick rebuild reintroduces, and `TrayStateHandle::store` cannot see it
    /// because an identical snapshot compares equal.
    tray_publishes: u64,
    tray_rule_count: usize,
    tray_source_count: usize,
    tray_reload_error: Option<String>,
    tray_config_warnings: Vec<ConfigWarning>,
    plugin_statuses: Vec<crate::plugins::PluginStatus>,
    plugin_fingerprint: Option<u64>,
    autostart_status: AutostartStatus,
    state_path: Option<PathBuf>,
    last_clipboard_path: Option<PathBuf>,
    history_writer: Option<HistoryWriter>,
    history_index: VecDeque<(Uuid, usize)>,
    paused: bool,
}

#[derive(Debug, Clone)]
struct RecentTransform {
    transform_id: Uuid,
    result: String,
    transformed_at: SystemTime,
    rules: Vec<TrayRule>,
}

#[derive(Debug, Clone)]
struct RecentExternalCopy {
    fingerprint: ClipboardFingerprint,
    transformed_at: Instant,
}

struct PreparedRuleJob {
    content: ClipboardItem,
    fingerprint: ClipboardFingerprint,
    change_count: Option<u64>,
    disabled_rule_ids: BTreeSet<String>,
    epoch: u64,
    profiled_at: Option<Instant>,
    profiled_bytes: usize,
}

struct PendingRuleJob {
    id: u64,
    fingerprint: ClipboardFingerprint,
    change_count: Option<u64>,
    epoch: u64,
    profiled_at: Option<Instant>,
    profiled_bytes: usize,
}

#[derive(Debug)]
pub enum AppEvent {
    ClipboardChanged(ClipboardItem),
    ConfigReloaded(reload::ReloadOutcome),
    UserCommand(AppCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCommand {
    Undo {
        transform_id: Uuid,
    },
    RestoreHistory {
        transform_id: Uuid,
    },
    EditRule {
        rule_id: Option<String>,
    },
    DisableRule {
        rule_id: String,
        seconds: u64,
    },
    ReloadConfig,
    OpenConfig,
    RevealConfig,
    CopyConfigPath,
    /// Copies arbitrary host-rendered text (plugin ids, diagnostics commands).
    CopyText {
        text: String,
    },
    /// Reveals a file in the platform file manager (plugin modules).
    RevealPath {
        path: PathBuf,
    },
    ClearHistory,
    SetAutostart(bool),
    SetPaused(bool),
    Quit,
}

impl From<ct_tray::TrayAction> for AppCommand {
    fn from(action: ct_tray::TrayAction) -> Self {
        use ct_tray::TrayAction as Action;

        match action {
            Action::RestoreHistory { transform_id } => Self::RestoreHistory { transform_id },
            Action::EditRule { rule_id } => Self::EditRule { rule_id },
            Action::DisableRule { rule_id, seconds } => Self::DisableRule { rule_id, seconds },
            Action::ReloadConfig => Self::ReloadConfig,
            Action::OpenConfig => Self::OpenConfig,
            Action::RevealConfig => Self::RevealConfig,
            Action::CopyConfigPath => Self::CopyConfigPath,
            Action::CopyText { text } => Self::CopyText { text },
            Action::RevealPath { path } => Self::RevealPath { path },
            Action::ClearHistory => Self::ClearHistory,
            Action::SetAutostart(enabled) => Self::SetAutostart(enabled),
            Action::SetPaused(paused) => Self::SetPaused(paused),
            Action::Quit => Self::Quit,
        }
    }
}

impl From<ct_notifications::NotificationAction> for AppCommand {
    fn from(action: ct_notifications::NotificationAction) -> Self {
        use ct_notifications::NotificationAction as Action;

        match action {
            Action::Undo { transform_id } => Self::Undo { transform_id },
            Action::EditRule { rule_id } => Self::EditRule { rule_id },
            Action::DisableRule { rule_id, seconds } => Self::DisableRule { rule_id, seconds },
            Action::ReloadConfig => Self::ReloadConfig,
            Action::OpenConfig => Self::OpenConfig,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEffect {
    OpenEditor {
        target: EditTarget,
        editor: Option<EditorConfig>,
    },
    ReloadConfig,
    OpenConfig(PathBuf),
    RevealConfig(PathBuf),
    SetAutostart(bool),
    Quit,
}

impl<C, N> Agent<C, N>
where
    C: ClipboardBackend,
    N: NotificationBackend,
{
    pub fn new(clipboard: C, notifications: N, config: ConfigDocument) -> Result<Self> {
        let engine = RuleEngine::compile(config.rules.clone())?;
        Self::new_with_engine(clipboard, notifications, config, engine)
    }

    pub fn new_with_engine(
        clipboard: C,
        notifications: N,
        mut config: ConfigDocument,
        engine: RuleEngine,
    ) -> Result<Self> {
        let app_matcher = config.config.app_matcher()?;
        let tray_rule_count = engine.rule_count();
        let required_formats = engine.required_formats().clone();
        let rule_worker = RuleWorker::start(engine)?;
        // The worker owns the compiled rules. Keeping the complete imported
        // RawRule tree in Agent duplicates large rule lists for no runtime
        // benefit; ConfigReloader retains the source document it needs for
        // change detection.
        config.rules.clear();
        logging::event("agent initialized");
        Ok(Self {
            clipboard,
            notifications,
            rule_worker,
            next_rule_job_id: 1,
            active_rule_job: None,
            queued_rule_job: None,
            rule_epoch: 0,
            required_formats,
            config,
            app_matcher,
            undo: UndoState::default(),
            write_guard: ClipboardWriteGuard::default(),
            disabled_rules: BTreeMap::new(),
            last_seen: None,
            last_transformed_external: None,
            edit_config_path: None,
            rule_sources: BTreeMap::new(),
            recent_transforms: VecDeque::new(),
            tray_state: None,
            tray_publishes: 0,
            tray_rule_count,
            tray_source_count: 1,
            tray_reload_error: None,
            tray_config_warnings: Vec::new(),
            plugin_statuses: Vec::new(),
            plugin_fingerprint: None,
            autostart_status: AutostartStatus::Unsupported,
            state_path: None,
            last_clipboard_path: None,
            history_writer: None,
            history_index: VecDeque::new(),
            paused: false,
        })
    }

    /// Gives the agent the handle the tray's menu source reads from.
    ///
    /// Publishing happens on every state change from here on, so the tray never
    /// has to be told to refresh and nothing is rebuilt on a timer.
    pub fn attach_tray_state(&mut self, handle: crate::platform::tray::TrayStateHandle) {
        handle.store(self.tray_snapshot());
        self.tray_state = Some(handle);
    }

    /// Writes current tray-visible state through to the shared handle.
    ///
    /// Driven from exactly one place — the end of a host tick that did work (see
    /// `Runtime::process_pending`). Every mutation during normal operation
    /// reaches the agent through that tick, so new mutation sites are covered
    /// without anyone remembering to annotate them. Do not add calls at
    /// individual field writes: scattering them is what makes staleness possible.
    pub(crate) fn publish_tray_state(&mut self) {
        if let Some(handle) = self.tray_state.clone() {
            self.tray_publishes += 1;
            handle.store(self.tray_snapshot());
        }
    }

    /// How many times tray state has been rebuilt and published.
    #[cfg(test)]
    pub(crate) fn tray_publish_count(&self) -> u64 {
        self.tray_publishes
    }

    pub fn set_edit_config_path(&mut self, path: PathBuf) {
        self.edit_config_path = Some(path);
    }

    pub fn set_rule_sources(&mut self, rule_sources: BTreeMap<String, RuleSource>) {
        self.rule_sources = rule_sources;
    }

    pub fn set_tray_source_count(&mut self, source_count: usize) {
        self.tray_source_count = source_count;
    }

    pub fn set_tray_config_warnings(&mut self, warnings: Vec<ConfigWarning>) {
        self.tray_config_warnings = warnings;
    }

    /// Records the initialized plugin statuses for tray and notifications.
    /// The fingerprint deduplicates "requires attention" notifications
    /// across hot reloads with unchanged issues.
    pub fn set_plugin_statuses(
        &mut self,
        statuses: Vec<crate::plugins::PluginStatus>,
        fingerprint: u64,
    ) {
        self.plugin_statuses = statuses;
        self.plugin_fingerprint = Some(fingerprint);
    }

    pub fn set_autostart_status(&mut self, status: AutostartStatus) {
        self.autostart_status = status;
    }

    pub fn load_persistent_state(&mut self, state_dir: PathBuf) -> Result<()> {
        let state_path = state_dir.join("state.json");
        let history_path = state_dir.join("history.cbor");
        let last_clipboard_path = state_dir.join("last-clipboard.cbor");
        if self.config.config.persist_last_clipboard {
            self.last_clipboard_path = Some(last_clipboard_path.clone());
            if let Err(error) = LastClipboardSnapshot::load(&last_clipboard_path) {
                logging::event(format!(
                    "last clipboard load failed, using a fresh snapshot: {error:#}"
                ));
                if let Err(error) = remove_corrupt_file(&last_clipboard_path) {
                    logging::event(format!("last clipboard removal failed: {error:#}"));
                }
            }
        } else {
            self.last_clipboard_path = None;
            if let Err(error) = remove_corrupt_file(&last_clipboard_path) {
                logging::event(format!("disabled last clipboard cleanup failed: {error:#}"));
            }
        }
        let mut state = match PersistentAppState::load(&state_path) {
            Ok(state) => state,
            Err(error) => {
                logging::event(format!("state load failed, using defaults: {error:#}"));
                if let Err(error) = quarantine_corrupt_file(&state_path) {
                    logging::event(format!("state quarantine failed: {error:#}"));
                }
                PersistentAppState {
                    version: PersistentAppState::VERSION,
                    ..PersistentAppState::default()
                }
            }
        };
        let mut history = match PersistentHistory::load(&history_path) {
            Ok(history) => history,
            Err(error) => {
                logging::event(format!(
                    "history load failed, using empty history: {error:#}"
                ));
                if let Err(error) = remove_corrupt_file(&history_path) {
                    logging::event(format!("history removal failed: {error:#}"));
                }
                PersistentHistory {
                    version: PersistentHistory::VERSION,
                    ..PersistentHistory::default()
                }
            }
        };
        let now_ms = unix_millis(SystemTime::now());
        state
            .disabled_rules_until_unix_ms
            .retain(|_, until| *until > now_ms);
        self.disabled_rules = state
            .disabled_rules_until_unix_ms
            .iter()
            .map(|(id, until)| (id.clone(), UNIX_EPOCH + Duration::from_millis(*until)))
            .collect();
        self.paused = state.paused;

        history.prune(
            self.config.config.recent_items_count,
            self.config.config.max_history_bytes,
        );
        self.recent_transforms = history
            .items
            .iter()
            .map(|record| RecentTransform {
                transform_id: record.transform.transform_id,
                result: tray_result_summary(&record.transform.transformed),
                transformed_at: UNIX_EPOCH + Duration::from_millis(record.transformed_at_unix_ms),
                rules: record
                    .rules
                    .iter()
                    .map(|rule| TrayRule {
                        id: rule.id.clone(),
                        label: rule.name.as_deref().unwrap_or(&rule.id).to_string(),
                    })
                    .collect(),
            })
            .collect();
        for record in history.items.iter().rev() {
            self.undo.remember(record.transform.clone());
        }
        self.history_index = history
            .items
            .iter()
            .map(|record| (record.transform.transform_id, record.size_bytes()))
            .collect();
        self.state_path = Some(state_path);
        self.history_writer = Some(HistoryWriter::start(history_path, history)?);
        self.persist_state_best_effort();
        self.prune_persistent_history();
        Ok(())
    }

    pub fn tray_snapshot(&self) -> TraySnapshot {
        TraySnapshot {
            recent: self
                .recent_transforms
                .iter()
                .take(self.config.config.recent_items_count)
                .map(|item| TrayRecentItem {
                    transform_id: item.transform_id,
                    result: item.result.clone(),
                    transformed_at: item.transformed_at,
                    rules: item.rules.clone(),
                    can_undo: self.undo.contains(item.transform_id),
                })
                .collect(),
            rule_count: self.tray_rule_count,
            source_count: self.tray_source_count,
            reload_error: self.tray_reload_error.clone(),
            config_warnings: self.tray_config_warnings.clone(),
            plugins: self
                .plugin_statuses
                .iter()
                .map(|status| crate::platform::tray::TrayPlugin {
                    id: status.id.clone(),
                    name: status
                        .manifest
                        .as_ref()
                        .map(|manifest| manifest.name.clone())
                        .unwrap_or_else(|| status.id.clone()),
                    state: status.state.as_str(),
                    issues: status
                        .issues
                        .iter()
                        .map(|issue| issue.summary.clone())
                        .collect(),
                    requires_attention: status.requires_attention(),
                    module_path: status.path.clone(),
                })
                .collect(),
            config_path: self.edit_config_path.clone(),
            disable_for: self.config.config.disable_for,
            autostart: self.autostart_status.clone(),
            paused: self.paused,
        }
    }

    pub fn deliver_startup_notification(
        &mut self,
        config_path: &Path,
        rule_count: usize,
    ) -> Result<()> {
        self.deliver_startup_best_effort(StartupNotification {
            notification_id: "clipboard-transformer-startup".to_string(),
            title: "Clipboard Transformer is running".to_string(),
            body: format!(
                "{rule_count} valid {} active from {}",
                pluralize_rules(rule_count),
                crate::config::short_path_for_display(config_path)
            ),
            edit_target: Some(EditTarget {
                path: config_path.display().to_string(),
                line: None,
            }),
            reload_request_path: None,
        });
        Ok(())
    }

    pub fn run_once_from_clipboard(&mut self) -> Result<Option<Uuid>> {
        logging::event("agent run-once reading clipboard");
        let Some(content) = self.read_clipboard_observation(None)? else {
            logging::event("agent run-once clipboard empty or unsupported");
            return Ok(None);
        };
        self.handle_clipboard_change(content)
    }

    pub fn handle_event(&mut self, event: AppEvent) -> Result<Vec<AppEffect>> {
        match event {
            AppEvent::ClipboardChanged(content) => {
                self.queue_clipboard_change(content)?;
                Ok(Vec::new())
            }
            AppEvent::ConfigReloaded(outcome) => {
                self.handle_reload_outcome(outcome)?;
                Ok(Vec::new())
            }
            AppEvent::UserCommand(command) => self.handle_user_command(command),
        }
    }

    pub(crate) fn poll_clipboard(
        &mut self,
        last_change_count: &mut Option<u64>,
    ) -> Result<Option<ClipboardItem>> {
        match self.clipboard.change_count()? {
            Some(change_count) if *last_change_count == Some(change_count) => {}
            Some(change_count) => {
                let first_poll = last_change_count.is_none();
                if first_poll {
                    // Seed both the change count and current payload without
                    // transforming clipboard content that predates this
                    // launch. Remembering the payload also makes a persisted
                    // Undo entry immediately available when it still matches.
                    self.last_seen = self
                        .read_clipboard_observation(Some(change_count))?
                        .map(|item| item.fingerprint());
                    *last_change_count = Some(change_count);
                    return Ok(None);
                }
                *last_change_count = Some(change_count);
                return self.read_clipboard_change(Some(change_count));
            }
            None => return self.read_clipboard_change(None),
        }
        Ok(None)
    }

    fn read_clipboard_change(
        &mut self,
        expected_change_count: Option<u64>,
    ) -> Result<Option<ClipboardItem>> {
        if let Some(content) = self.read_clipboard_observation(expected_change_count)? {
            if self.last_seen != Some(content.fingerprint()) {
                return Ok(Some(content));
            }
        }
        Ok(None)
    }

    pub fn handle_clipboard_change(&mut self, content: ClipboardItem) -> Result<Option<Uuid>> {
        self.queued_rule_job = None;
        let Some(job) = self.prepare_rule_job(content)? else {
            return Ok(None);
        };
        let result = self
            .rule_worker
            .apply_blocking(job.content, job.disabled_rule_ids)?;
        let Some(result) = result else {
            logging::event("no rule matched clipboard content");
            return Ok(None);
        };
        self.apply_transform_result(result)
    }

    fn queue_clipboard_change(&mut self, content: ClipboardItem) -> Result<()> {
        // Keep at most one in-flight job and one latest pending payload. A new
        // clipboard observation replaces an older queued observation.
        self.queued_rule_job = None;
        let Some(job) = self.prepare_rule_job(content)? else {
            return Ok(());
        };
        if self.active_rule_job.is_some() {
            self.queued_rule_job = Some(job);
        } else {
            self.start_rule_job(job)?;
        }
        Ok(())
    }

    fn prepare_rule_job(&mut self, content: ClipboardItem) -> Result<Option<PreparedRuleJob>> {
        let fingerprint = content.fingerprint();
        let change_count = match self.write_guard.classify_change(&content) {
            ClipboardChangeKind::OwnWrite => {
                logging::event("clipboard change classified as own write");
                self.last_seen = Some(fingerprint);
                return Ok(None);
            }
            ClipboardChangeKind::External => {
                logging::event("clipboard change classified as external");
                if !self.app_matcher.allows_app(content.source_app()) {
                    logging::event("clipboard change ignored for source app");
                    self.last_seen = Some(fingerprint);
                    return Ok(None);
                }
                let change_count = self.persist_last_clipboard_best_effort(&content);
                let notification_ids = self
                    .undo
                    .notification_ids()
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                self.remove_notifications_best_effort(notification_ids);
                change_count
            }
        };

        if self.should_ignore_double_copy(&content) {
            logging::event("clipboard ignored as intentional double copy");
            self.last_seen = Some(fingerprint);
            self.deliver_double_copy_ignored_notification()?;
            return Ok(None);
        }

        self.last_seen = Some(fingerprint);
        let max_item_bytes = self.config.config.max_item_bytes;
        if max_item_bytes > 0 && content.size_bytes() as u128 > u128::from(max_item_bytes) {
            logging::event(format!(
                "clipboard item ignored size_bytes={} max_item_bytes={max_item_bytes}",
                content.size_bytes()
            ));
            return Ok(None);
        }
        if self.paused {
            logging::event("clipboard transformation skipped while paused");
            return Ok(None);
        }
        let now = SystemTime::now();
        self.disabled_rules.retain(|_, until| *until > now);
        let disabled_rule_ids = self.disabled_rules.keys().cloned().collect::<BTreeSet<_>>();
        if !self
            .required_formats
            .iter()
            .any(|format| format.as_str() == "*" || content.bytes(format).is_some())
        {
            logging::event("clipboard item has no representation requested by active rules");
            return Ok(None);
        }
        Ok(Some(PreparedRuleJob {
            profiled_at: performance_profiling_enabled().then(Instant::now),
            profiled_bytes: content.size_bytes(),
            content,
            fingerprint,
            change_count,
            disabled_rule_ids,
            epoch: self.rule_epoch,
        }))
    }

    fn start_rule_job(&mut self, job: PreparedRuleJob) -> Result<()> {
        let job_id = self.next_rule_job_id;
        self.next_rule_job_id = self.next_rule_job_id.wrapping_add(1).max(1);
        self.rule_worker
            .submit(job_id, job.content, job.disabled_rule_ids)?;
        self.active_rule_job = Some(PendingRuleJob {
            id: job_id,
            fingerprint: job.fingerprint,
            change_count: job.change_count,
            epoch: job.epoch,
            profiled_at: job.profiled_at,
            profiled_bytes: job.profiled_bytes,
        });
        Ok(())
    }

    /// Returns whether any completion was handled, so the caller knows a tick
    /// actually changed state.
    pub(crate) fn poll_rule_results(&mut self) -> Result<bool> {
        let mut handled = false;
        while let Some(completion) = self.rule_worker.try_completion()? {
            self.handle_rule_completion(completion)?;
            handled = true;
        }
        Ok(handled)
    }

    /// Blocks until no rule job is outstanding, processing each completion with
    /// the same code path as the polling host loop.
    ///
    /// The timeout only guards against a genuinely stuck worker: it is not an
    /// assertion about how fast the worker replies, so it can be generous
    /// without making the wait slower in the normal case.
    #[cfg(test)]
    fn settle_rule_results(&mut self, timeout: std::time::Duration) -> Result<()> {
        self.poll_rule_results()?;
        while self.active_rule_job.is_some() || self.queued_rule_job.is_some() {
            let Some(completion) = self.rule_worker.wait_completion(timeout)? else {
                anyhow::bail!("rule worker did not become idle within {timeout:?}");
            };
            self.handle_rule_completion(completion)?;
        }
        Ok(())
    }

    fn handle_rule_completion(&mut self, completion: RuleWorkerCompletion) -> Result<()> {
        {
            let Some(active) = self.active_rule_job.take() else {
                logging::event(format!(
                    "rule result ignored without active job job_id={}",
                    completion.job_id
                ));
                return Ok(());
            };
            if active.id != completion.job_id {
                logging::event(format!(
                    "out-of-order rule result ignored job_id={} active_job_id={}",
                    completion.job_id, active.id
                ));
                self.active_rule_job = Some(active);
                return Ok(());
            }

            let has_newer_job = self.queued_rule_job.is_some();
            let still_current = active.epoch == self.rule_epoch
                && !self.paused
                && self.last_seen == Some(active.fingerprint);
            match completion.outcome {
                Ok(Some(result)) if still_current && !has_newer_job => {
                    log_profile_elapsed("rule-worker", active.profiled_at, active.profiled_bytes);
                    match self.clipboard_still_matches(&active) {
                        Ok(true) => {
                            self.apply_transform_result(result)?;
                        }
                        Ok(false) => logging::event(format!(
                            "stale rule result discarded job_id={} clipboard changed",
                            active.id
                        )),
                        Err(error) => logging::event(format!(
                            "rule result discarded because clipboard verification failed: {error:#}"
                        )),
                    }
                }
                Ok(Some(_)) => logging::event(format!(
                    "stale rule result discarded job_id={} newer_job={has_newer_job}",
                    active.id
                )),
                Ok(None) => logging::event("no rule matched clipboard content"),
                Err(error) => logging::event(format!("rule transform failed: {error}")),
            }

            if let Some(next) = self.queued_rule_job.take() {
                self.start_rule_job(next)?;
            }
        }
        Ok(())
    }

    fn apply_transform_result(&mut self, result: TransformResult) -> Result<Option<Uuid>> {
        let profile_started = performance_profiling_enabled().then(Instant::now);
        let profile_bytes = result
            .before
            .size_bytes()
            .saturating_add(result.after.size_bytes());
        let max_history_bytes = self.config.config.max_history_bytes;
        if self.config.config.recent_items_count > 0 && max_history_bytes > 0 {
            let record_size = result
                .before
                .size_bytes()
                .saturating_add(result.after.size_bytes());
            if record_size as u128 > u128::from(max_history_bytes) {
                logging::event(format!(
                    "clipboard transform skipped history_record_bytes={record_size} \
                     max_history_bytes={max_history_bytes}"
                ));
                return Ok(None);
            }
        }
        let transform_id = Uuid::new_v4();
        let notification_id = format!("clipboard-transformer-{transform_id}");
        let notification_title = notification_title(&result);
        let notification_body = notification_body(&result);
        self.write_guard.mark_own_write(&result.after);
        self.clipboard.write(&result.after)?;
        self.last_seen = Some(result.after.fingerprint());
        self.last_transformed_external = Some(RecentExternalCopy {
            fingerprint: result.before.fingerprint(),
            transformed_at: Instant::now(),
        });
        logging::event(format!(
            "clipboard transformed transform_id={transform_id} rules={}",
            result.applied_rule_ids().collect::<Vec<_>>().join(",")
        ));

        let transformed_at = SystemTime::now();
        if self.config.config.recent_items_count > 0 {
            self.recent_transforms.push_front(RecentTransform {
                transform_id,
                result: tray_result_summary(&result.after),
                transformed_at,
                rules: result
                    .applied_rules
                    .iter()
                    .map(|rule| TrayRule {
                        id: rule.id.clone(),
                        label: rule.label().to_string(),
                    })
                    .collect(),
            });
            self.recent_transforms
                .truncate(self.config.config.recent_items_count);
        }

        let last_transform = LastTransform {
            transform_id,
            rule_id: result.applied_rules.last().map(|rule| rule.id.clone()),
            previous: result.before,
            transformed: result.after.clone(),
            notification_id: notification_id.clone(),
        };
        self.undo.remember(last_transform.clone());
        let removed = self
            .undo
            .truncate(self.config.config.recent_items_count.max(1));
        self.remove_notifications_best_effort(removed);

        self.deliver_transform_best_effort(TransformNotification {
            notification_id: notification_id.clone(),
            transform_id,
            rule_id: result.applied_rules.last().map(|rule| rule.id.clone()),
            title: notification_title,
            body: notification_body,
            disable_for_seconds: (self.config.config.disable_for > 0)
                .then_some(self.config.config.disable_for),
            edit_target: result
                .applied_rules
                .last()
                .and_then(|rule| self.rule_edit_target(&rule.id))
                .or_else(|| self.config_edit_target()),
        });

        if self.config.config.recent_items_count > 0 {
            let record = HistoryRecord {
                transform: last_transform,
                transformed_at_unix_ms: unix_millis(transformed_at),
                rules: result
                    .applied_rules
                    .iter()
                    .map(|rule| HistoryRule {
                        id: rule.id.clone(),
                        name: rule.name.clone(),
                    })
                    .collect(),
            };
            self.history_index
                .push_front((record.transform.transform_id, record.size_bytes()));
            self.prune_persistent_history_index();
            if let Some(writer) = &self.history_writer {
                writer.append_and_prune(
                    record,
                    self.config.config.recent_items_count,
                    self.config.config.max_history_bytes,
                );
            }
        }

        log_profile_elapsed("apply-result", profile_started, profile_bytes);
        Ok(Some(transform_id))
    }

    pub fn disable_rule(&mut self, rule_id: String, seconds: u64) {
        if seconds > 0 {
            self.disabled_rules
                .insert(rule_id, SystemTime::now() + Duration::from_secs(seconds));
            self.rule_epoch = self.rule_epoch.wrapping_add(1);
            self.queued_rule_job = None;
            self.persist_state_best_effort();
        }
    }

    fn handle_user_command(&mut self, command: AppCommand) -> Result<Vec<AppEffect>> {
        match command {
            AppCommand::Undo { transform_id } => {
                self.undo(transform_id)?;
                Ok(Vec::new())
            }
            AppCommand::RestoreHistory { transform_id } => {
                self.restore_history(transform_id)?;
                Ok(Vec::new())
            }
            AppCommand::EditRule { rule_id } => Ok(rule_id
                .as_deref()
                .and_then(|rule_id| self.rule_edit_target(rule_id))
                .or_else(|| self.config_edit_target())
                .map(|target| AppEffect::OpenEditor {
                    target,
                    editor: self.config.config.editor.clone(),
                })
                .into_iter()
                .collect()),
            AppCommand::DisableRule { rule_id, seconds } => {
                self.disable_rule(rule_id, seconds);
                Ok(Vec::new())
            }
            AppCommand::ReloadConfig => Ok(vec![AppEffect::ReloadConfig]),
            AppCommand::OpenConfig => Ok(self
                .edit_config_path
                .clone()
                .map(AppEffect::OpenConfig)
                .into_iter()
                .collect()),
            AppCommand::RevealConfig => Ok(self
                .edit_config_path
                .clone()
                .map(AppEffect::RevealConfig)
                .into_iter()
                .collect()),
            AppCommand::CopyConfigPath => {
                if let Some(path) = &self.edit_config_path {
                    let content = ClipboardItem::from_text(path.display().to_string());
                    self.write_guard.mark_own_write(&content);
                    self.clipboard.write(&content)?;
                    self.last_seen = Some(content.fingerprint());
                }
                Ok(Vec::new())
            }
            AppCommand::CopyText { text } => {
                let content = ClipboardItem::from_text(text);
                self.write_guard.mark_own_write(&content);
                self.clipboard.write(&content)?;
                self.last_seen = Some(content.fingerprint());
                Ok(Vec::new())
            }
            AppCommand::RevealPath { path } => Ok(vec![AppEffect::RevealConfig(path)]),
            AppCommand::ClearHistory => {
                let removed = self.undo.clear();
                self.remove_notifications_best_effort(removed);
                self.recent_transforms.clear();
                self.history_index.clear();
                if let Some(writer) = &self.history_writer {
                    writer.clear();
                }
                Ok(Vec::new())
            }
            AppCommand::SetAutostart(enabled) => Ok(vec![AppEffect::SetAutostart(enabled)]),
            AppCommand::SetPaused(paused) => {
                if self.paused != paused {
                    self.paused = paused;
                    self.rule_epoch = self.rule_epoch.wrapping_add(1);
                    self.queued_rule_job = None;
                }
                self.persist_state_best_effort();
                Ok(Vec::new())
            }
            AppCommand::Quit => Ok(vec![AppEffect::Quit]),
        }
    }

    fn undo(&mut self, transform_id: Uuid) -> Result<()> {
        let Some(current) = self.read_clipboard_payload()? else {
            return Ok(());
        };
        let Some(previous) = self.undo.undo(transform_id, &current) else {
            if let Some(notification_id) =
                self.undo.notification_id(transform_id).map(str::to_string)
            {
                self.remove_notification_best_effort(&notification_id);
            }
            logging::event(format!(
                "undo ignored transform_id={transform_id} reason=stale-or-missing"
            ));
            return Ok(());
        };

        self.write_guard.mark_own_write(&previous);
        self.clipboard.write(&previous)?;
        self.last_seen = Some(previous.fingerprint());
        self.remove_notification_best_effort(&format!("clipboard-transformer-{transform_id}"));
        logging::event(format!("undo applied transform_id={transform_id}"));
        Ok(())
    }

    fn restore_history(&mut self, transform_id: Uuid) -> Result<()> {
        let Some(previous) = self.undo.restore(transform_id) else {
            logging::event(format!(
                "history restore ignored transform_id={transform_id} reason=missing"
            ));
            return Ok(());
        };

        self.write_guard.mark_own_write(&previous);
        self.clipboard.write(&previous)?;
        self.last_seen = Some(previous.fingerprint());
        self.remove_notification_best_effort(&format!("clipboard-transformer-{transform_id}"));
        logging::event(format!("history restored transform_id={transform_id}"));
        Ok(())
    }

    fn should_ignore_double_copy(&mut self, content: &ClipboardItem) -> bool {
        let timeout = Duration::from_secs(self.config.config.double_copy_window);
        if timeout.is_zero() {
            return false;
        }

        let now = Instant::now();
        if self
            .last_transformed_external
            .as_ref()
            .is_some_and(|recent| now.duration_since(recent.transformed_at) > timeout)
        {
            self.last_transformed_external = None;
        }

        self.last_transformed_external
            .as_ref()
            .is_some_and(|recent| recent.fingerprint == content.fingerprint())
    }

    fn deliver_double_copy_ignored_notification(&mut self) -> Result<()> {
        let timeout = Duration::from_secs(self.config.config.double_copy_window);
        self.deliver_startup_best_effort(StartupNotification {
            notification_id: "clipboard-transformer-double-copy-ignored".to_string(),
            title: "Rules skipped".to_string(),
            body: format!(
                "Copied again within {}; kept original.",
                human_duration(timeout)
            ),
            edit_target: self.config_edit_target(),
            reload_request_path: None,
        });
        Ok(())
    }

    fn handle_reload_outcome(&mut self, outcome: reload::ReloadOutcome) -> Result<()> {
        match outcome {
            reload::ReloadOutcome::Applied {
                config,
                engine,
                rule_count,
                watched_sources,
                rule_sources,
                warnings,
                plugin_statuses,
                plugin_fingerprint,
            } => {
                let mut config = *config;
                let app_matcher = config.config.app_matcher()?;
                self.notifications
                    .configure_disable_for(config.config.disable_for)?;
                self.required_formats = engine.required_formats().clone();
                self.rule_worker.replace_engine(engine)?;
                self.rule_epoch = self.rule_epoch.wrapping_add(1);
                self.queued_rule_job = None;
                self.app_matcher = app_matcher;
                config.rules.clear();
                self.configure_last_clipboard_persistence(config.config.persist_last_clipboard);
                self.config = config;
                self.tray_rule_count = rule_count;
                self.tray_source_count = watched_sources.len();
                self.rule_sources = rule_sources;
                self.tray_reload_error = None;
                self.tray_config_warnings = warnings;
                let plugins_changed = self.plugin_fingerprint != Some(plugin_fingerprint);
                self.plugin_statuses = plugin_statuses;
                self.plugin_fingerprint = Some(plugin_fingerprint);
                self.recent_transforms
                    .truncate(self.config.config.recent_items_count);
                self.prune_persistent_history();
                self.deliver_startup_best_effort(StartupNotification {
                    notification_id: "clipboard-transformer-config-reloaded".to_string(),
                    title: "Clipboard Transformer config reloaded".to_string(),
                    body: format!(
                        "{rule_count} valid {} active from {} watched file(s)",
                        pluralize_rules(rule_count),
                        watched_sources.len()
                    ),
                    edit_target: self.config_edit_target(),
                    reload_request_path: None,
                });
                if plugins_changed {
                    self.deliver_plugin_attention_notification();
                }
            }
            reload::ReloadOutcome::Unchanged => {
                self.tray_reload_error = None;
            }
            reload::ReloadOutcome::Failed {
                error,
                reload_request_path,
            } => {
                logging::event(format!("config reload failed: {error}"));
                self.tray_reload_error = Some(error.clone());
                self.deliver_startup_best_effort(StartupNotification {
                    notification_id: "clipboard-transformer-config-reload-failed".to_string(),
                    title: "Clipboard Transformer config reload failed".to_string(),
                    body: format!("Keeping the last valid config in memory. {error}"),
                    edit_target: self.config_edit_target(),
                    reload_request_path,
                });
            }
        }
        Ok(())
    }

    /// One deduplicated notification when plugins need the user's attention.
    /// Callers gate on the issue fingerprint so repeated reloads with the
    /// same issues stay quiet.
    pub fn deliver_plugin_attention_notification(&mut self) {
        let attention: Vec<&crate::plugins::PluginStatus> = self
            .plugin_statuses
            .iter()
            .filter(|status| status.requires_attention())
            .collect();
        if attention.is_empty() {
            return;
        }
        let ids = attention
            .iter()
            .map(|status| status.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let count = attention.len();
        self.deliver_startup_best_effort(StartupNotification {
            notification_id: "clipboard-transformer-plugins-attention".to_string(),
            title: "Clipboard Transformer plugins need attention".to_string(),
            body: format!(
                "{count} plugin{} require{} attention: {ids}. Run `clipboard-transformer plugin doctor` for details.",
                if count == 1 { "" } else { "s" },
                if count == 1 { "s" } else { "" },
            ),
            edit_target: self.config_edit_target(),
            reload_request_path: None,
        });
    }

    fn config_edit_target(&self) -> Option<EditTarget> {
        self.edit_config_path.as_ref().map(|path| EditTarget {
            path: path.display().to_string(),
            line: None,
        })
    }

    fn rule_edit_target(&self, rule_id: &str) -> Option<EditTarget> {
        self.rule_sources.get(rule_id).map(|source| EditTarget {
            path: source.path.display().to_string(),
            line: Some(source.line),
        })
    }

    fn deliver_startup_best_effort(&mut self, notification: StartupNotification) {
        if let Err(error) = self.notifications.deliver_startup(notification) {
            logging::event(format!("notification delivery failed: {error:#}"));
        }
    }

    fn deliver_transform_best_effort(&mut self, notification: TransformNotification) {
        if let Err(error) = self.notifications.deliver_transform(notification) {
            logging::event(format!("notification delivery failed: {error:#}"));
        }
    }

    fn remove_notification_best_effort(&mut self, notification_id: &str) {
        if let Err(error) = self.notifications.remove_delivered(notification_id) {
            logging::event(format!(
                "notification removal failed notification_id={notification_id}: {error:#}"
            ));
        }
    }

    fn remove_notifications_best_effort(
        &mut self,
        notification_ids: impl IntoIterator<Item = String>,
    ) {
        for notification_id in notification_ids {
            self.remove_notification_best_effort(&notification_id);
        }
    }

    fn persist_state_best_effort(&self) {
        let Some(path) = &self.state_path else {
            return;
        };
        let state = PersistentAppState {
            version: PersistentAppState::VERSION,
            paused: self.paused,
            disabled_rules_until_unix_ms: self
                .disabled_rules
                .iter()
                .map(|(id, until)| (id.clone(), unix_millis(*until)))
                .collect(),
        };
        if let Err(error) = state.save(path) {
            logging::event(format!("state save failed: {error:#}"));
        }
    }

    fn read_clipboard_observation(
        &mut self,
        expected_change_count: Option<u64>,
    ) -> Result<Option<ClipboardItem>> {
        let observation_started = performance_profiling_enabled().then(Instant::now);
        let change_count_before = match expected_change_count {
            Some(change_count) => Some(change_count),
            None => self.clipboard.change_count()?,
        };
        let metadata = self.clipboard.metadata()?;
        log_profile_elapsed("metadata-read", observation_started, 0);
        if metadata.is_ignored() {
            logging::event("clipboard observation ignored from native metadata");
            return Ok(None);
        }
        if !self.app_matcher.allows_app(metadata.source_app()) {
            logging::event("clipboard observation ignored for source app before payload read");
            return Ok(None);
        }
        let source_app = metadata.source_app().cloned();
        let payload_started = performance_profiling_enabled().then(Instant::now);
        let item = self
            .read_clipboard_payload()?
            .map(|item| item.with_optional_source_app(source_app));
        log_profile_elapsed(
            "payload-read",
            payload_started,
            item.as_ref().map_or(0, ClipboardItem::size_bytes),
        );
        if change_count_before.is_some() && self.clipboard.change_count()? != change_count_before {
            logging::event("clipboard changed between metadata and payload reads");
            return Ok(None);
        }
        Ok(item)
    }

    fn read_clipboard_payload(&mut self) -> Result<Option<ClipboardItem>> {
        if self.config.config.persist_last_clipboard
            || self
                .required_formats
                .iter()
                .any(|format| format.as_str() == "*")
        {
            self.clipboard
                .read_limited(self.config.config.max_item_bytes)
        } else {
            self.clipboard
                .read_formats_limited(&self.required_formats, self.config.config.max_item_bytes)
        }
    }

    fn clipboard_still_matches(&mut self, active: &PendingRuleJob) -> Result<bool> {
        if let Some(expected) = active.change_count {
            return Ok(self.clipboard.change_count()? == Some(expected));
        }
        Ok(self
            .read_clipboard_payload()?
            .is_some_and(|current| current.fingerprint() == active.fingerprint))
    }

    fn prune_persistent_history(&mut self) {
        self.prune_persistent_history_index();
        if let Some(writer) = &self.history_writer {
            writer.prune(
                self.config.config.recent_items_count,
                self.config.config.max_history_bytes,
            );
        }
    }

    fn prune_persistent_history_index(&mut self) {
        let max_items = self.config.config.recent_items_count;
        if max_items == 0 {
            self.history_index.clear();
            self.recent_transforms.clear();
            let removed = self.undo.truncate(1);
            self.remove_notifications_best_effort(removed);
        } else {
            self.history_index.truncate(max_items);
            let max_bytes = self.config.config.max_history_bytes;
            if max_bytes > 0 {
                let mut total = self.history_index.iter().fold(0u128, |total, (_, size)| {
                    total.saturating_add(*size as u128)
                });
                while total > u128::from(max_bytes) {
                    let Some((_, removed_size)) = self.history_index.pop_back() else {
                        break;
                    };
                    total = total.saturating_sub(removed_size as u128);
                }
            }
            let retained = self
                .history_index
                .iter()
                .map(|(transform_id, _)| *transform_id)
                .collect::<BTreeSet<_>>();
            self.recent_transforms
                .retain(|item| retained.contains(&item.transform_id));
            let removed = self.undo.retain_ids(&retained);
            self.remove_notifications_best_effort(removed);
        }
    }

    #[cfg(test)]
    fn flush_history_writer(&self) {
        if let Some(writer) = &self.history_writer {
            writer.flush();
        }
    }

    fn persist_last_clipboard_best_effort(&mut self, item: &ClipboardItem) -> Option<u64> {
        let Some(path) = self.last_clipboard_path.clone() else {
            return self.clipboard.change_count().ok().flatten();
        };
        let change_count = match self.clipboard.change_count() {
            Ok(value) => value,
            Err(error) => {
                logging::event(format!(
                    "last clipboard change count unavailable: {error:#}"
                ));
                None
            }
        };
        if let Err(error) = LastClipboardSnapshot::save_item(
            &path,
            item,
            unix_millis(SystemTime::now()),
            change_count,
        ) {
            logging::event(format!("last clipboard save failed: {error:#}"));
        }
        change_count
    }

    fn configure_last_clipboard_persistence(&mut self, enabled: bool) {
        if !enabled {
            if let Some(path) = self
                .state_path
                .as_deref()
                .and_then(Path::parent)
                .map(|state_dir| state_dir.join("last-clipboard.cbor"))
            {
                if let Err(error) = remove_corrupt_file(&path) {
                    logging::event(format!("disabled last clipboard cleanup failed: {error:#}"));
                }
            }
            self.last_clipboard_path = None;
            return;
        }
        let Some(state_dir) = self.state_path.as_deref().and_then(Path::parent) else {
            return;
        };
        let path = state_dir.join("last-clipboard.cbor");
        if let Err(error) = LastClipboardSnapshot::load(&path) {
            logging::event(format!(
                "last clipboard load failed after reload, using a fresh snapshot: {error:#}"
            ));
            if let Err(error) = remove_corrupt_file(&path) {
                logging::event(format!("last clipboard removal failed: {error:#}"));
            }
        }
        self.last_clipboard_path = Some(path);
    }
}

fn performance_profiling_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("CLIPBOARD_TRANSFORMER_PROFILE_PIPELINE").is_some_and(|value| value != "0")
    })
}

fn log_profile_elapsed(stage: &str, started: Option<Instant>, bytes: usize) {
    if let Some(started) = started {
        logging::event(format!(
            "performance stage={stage} elapsed_us={} payload_bytes={bytes}",
            started.elapsed().as_micros()
        ));
    }
}

fn unix_millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn tray_result_summary(content: &ClipboardItem) -> String {
    let value = content
        .text()
        .or_else(|| {
            content
                .text_representations()
                .next()
                .map(|(_, value)| value)
        })
        .unwrap_or("Converted clipboard content");
    let single_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = single_line.chars();
    let summary = chars.by_ref().take(80).collect::<String>();
    if chars.next().is_some() {
        format!("{summary}…")
    } else {
        summary
    }
}

fn notification_title(result: &TransformResult) -> String {
    match result.applied_rules.as_slice() {
        [rule] => format!("Rule {} has been applied", rule.label()),
        rules => format!("{} rules have been applied", rules.len()),
    }
}

fn notification_body(result: &TransformResult) -> String {
    match result.applied_rules.as_slice() {
        [_rule] => result
            .message
            .clone()
            .unwrap_or_else(|| "Clipboard content was transformed.".into()),
        rules => rule_labels(rules).join(", "),
    }
}

fn rule_labels(rules: &[AppliedRule]) -> Vec<String> {
    rules.iter().map(|rule| rule.label().to_string()).collect()
}

fn pluralize_rules(count: usize) -> &'static str {
    if count == 1 {
        "rule"
    } else {
        "rules"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use ct_clipboard::{ClipboardMetadata, ClipboardSourceApp, MemoryClipboardBackend};
    use ct_core::{AppMode, RawRule};
    use ct_notifications::NoopNotificationBackend;

    #[derive(Debug)]
    struct SequencedClipboard {
        inner: MemoryClipboardBackend,
        change_count: u64,
    }

    struct MetadataFilteredClipboard {
        source_app: ClipboardSourceApp,
        payload_reads: std::rc::Rc<std::cell::Cell<usize>>,
    }

    #[derive(Default)]
    struct SelectiveClipboard {
        full_reads: usize,
        selective_reads: usize,
    }

    impl ClipboardBackend for SelectiveClipboard {
        fn read(&mut self) -> Result<Option<ClipboardItem>> {
            self.full_reads += 1;
            Ok(Some(ClipboardItem::from_text("bird")))
        }

        fn read_formats_limited(
            &mut self,
            _formats: &BTreeSet<ClipboardFormat>,
            _max_bytes: u64,
        ) -> Result<Option<ClipboardItem>> {
            self.selective_reads += 1;
            Ok(Some(ClipboardItem::from_text("bird")))
        }

        fn write(&mut self, _content: &ClipboardItem) -> Result<()> {
            Ok(())
        }
    }

    impl ClipboardBackend for MetadataFilteredClipboard {
        fn metadata(&mut self) -> Result<ClipboardMetadata> {
            Ok(ClipboardMetadata::readable(Some(self.source_app.clone())))
        }

        fn read(&mut self) -> Result<Option<ClipboardItem>> {
            self.payload_reads.set(self.payload_reads.get() + 1);
            Ok(Some(ClipboardItem::from_text("cat")))
        }

        fn write(&mut self, _content: &ClipboardItem) -> Result<()> {
            Ok(())
        }
    }

    impl SequencedClipboard {
        fn new(current: Option<ClipboardItem>) -> Self {
            Self {
                inner: MemoryClipboardBackend::new(current),
                change_count: 1,
            }
        }
    }

    impl ClipboardBackend for SequencedClipboard {
        fn change_count(&mut self) -> Result<Option<u64>> {
            Ok(Some(self.change_count))
        }

        fn read(&mut self) -> Result<Option<ClipboardItem>> {
            self.inner.read()
        }

        fn write(&mut self, item: &ClipboardItem) -> Result<()> {
            self.inner.write(item)?;
            self.change_count = self.change_count.saturating_add(1);
            Ok(())
        }
    }

    fn config() -> ConfigDocument {
        ConfigDocument {
            plugins: Default::default(),
            config: AppConfig::default(),
            rules: vec![RawRule {
                id: "rule".into(),
                name: Some("Cat cleanup".into()),
                from: Some("cat".into()),
                to: Some("dog".into()),
                ..RawRule::default()
            }],
        }
    }

    #[test]
    fn agent_drops_raw_rules_after_compilation() {
        let agent = Agent::new(
            MemoryClipboardBackend::new(None),
            NoopNotificationBackend::default(),
            config(),
        )
        .unwrap();

        assert!(agent.config.rules.is_empty());
        assert!(agent.tray_rule_count > 0);
    }

    fn wait_for_rule_worker(agent: &mut Agent<MemoryClipboardBackend, NoopNotificationBackend>) {
        // Blocks on the worker's completion channel rather than polling against
        // a wall-clock deadline, so a loaded machine cannot fail the test.
        agent
            .settle_rule_results(Duration::from_secs(60))
            .expect("rule worker settles");
    }

    /// Attaching must seed the state: the tray is built from this source before
    /// any event has happened.
    #[test]
    fn attaching_publishes_current_state_immediately() {
        let mut agent = Agent::new(
            MemoryClipboardBackend::new(None),
            NoopNotificationBackend::default(),
            config(),
        )
        .unwrap();
        agent.set_tray_source_count(7);

        let handle = crate::platform::tray::TrayStateHandle::new(agent.tray_snapshot());
        agent.attach_tray_state(handle.clone());

        // Reported through the menu, which is the only consumer of this state.
        assert!(!handle.source()().items.is_empty());
    }

    #[test]
    fn transform_writes_clipboard_and_delivers_notification() {
        let clipboard = MemoryClipboardBackend::new(Some(ClipboardItem::from_text("cat")));
        let notifications = NoopNotificationBackend::default();
        let mut agent = Agent::new(clipboard, notifications, config()).unwrap();

        assert!(agent.run_once_from_clipboard().unwrap().is_some());
        assert_eq!(agent.notifications.delivered.len(), 1);
        assert_eq!(
            agent.notifications.delivered[0].title,
            "Rule Cat cleanup has been applied"
        );
        assert_eq!(
            agent.notifications.delivered[0].body,
            "Clipboard content was transformed."
        );
    }

    #[test]
    fn selective_text_transform_never_reads_or_writes_unrequested_payloads() {
        let mut original = ClipboardItem::from_text("cat");
        let binary_format = ClipboardFormat::new("public.png");
        original.set_bytes(binary_format.clone(), vec![0x89, b'P', b'N', b'G']);
        let clipboard = MemoryClipboardBackend::new(Some(original.clone()));
        let notifications = NoopNotificationBackend::default();
        let mut agent = Agent::new(clipboard, notifications, config()).unwrap();
        let state_dir = tempfile::tempdir().unwrap();
        agent
            .load_persistent_state(state_dir.path().to_path_buf())
            .unwrap();

        let transform_id = agent.run_once_from_clipboard().unwrap().unwrap();
        let transformed = agent.clipboard.read().unwrap().unwrap();
        assert_eq!(transformed.text(), Some("dog"));
        assert_eq!(transformed.bytes(&binary_format), None);

        agent
            .handle_event(AppEvent::UserCommand(AppCommand::Undo { transform_id }))
            .unwrap();
        let restored = agent.clipboard.read().unwrap().unwrap();
        assert_eq!(restored, ClipboardItem::from_text("cat"));
    }

    #[test]
    fn asynchronous_transform_discards_a_stale_clipboard_result() {
        let clipboard = MemoryClipboardBackend::new(Some(ClipboardItem::from_text("cat")));
        let notifications = NoopNotificationBackend::default();
        let mut agent = Agent::new(clipboard, notifications, config()).unwrap();

        agent
            .handle_event(AppEvent::ClipboardChanged(ClipboardItem::from_text("cat")))
            .unwrap();
        agent
            .clipboard
            .write(&ClipboardItem::from_text("new value"))
            .unwrap();

        wait_for_rule_worker(&mut agent);

        assert!(agent.active_rule_job.is_none());
        assert_eq!(
            agent.clipboard.read().unwrap().unwrap().text(),
            Some("new value")
        );
        assert!(agent.notifications.delivered.is_empty());
    }

    #[test]
    fn asynchronous_transform_keeps_only_the_latest_queued_clipboard_item() {
        let clipboard = MemoryClipboardBackend::new(Some(ClipboardItem::from_text("cat")));
        let notifications = NoopNotificationBackend::default();
        let mut agent = Agent::new(clipboard, notifications, config()).unwrap();

        agent
            .handle_event(AppEvent::ClipboardChanged(ClipboardItem::from_text("cat")))
            .unwrap();
        let latest = ClipboardItem::from_text("cat cat");
        agent.clipboard.write(&latest).unwrap();
        agent
            .handle_event(AppEvent::ClipboardChanged(latest))
            .unwrap();

        wait_for_rule_worker(&mut agent);

        assert!(agent.active_rule_job.is_none());
        assert!(agent.queued_rule_job.is_none());
        assert_eq!(
            agent.clipboard.read().unwrap().unwrap().text(),
            Some("dog dog")
        );
        assert_eq!(agent.notifications.delivered.len(), 1);
    }

    #[test]
    fn ignored_source_app_skips_processing() {
        let clipboard = MemoryClipboardBackend::new(None);
        let notifications = NoopNotificationBackend::default();
        let config = ConfigDocument {
            plugins: Default::default(),
            config: AppConfig {
                apps: vec!["com.example.Ignored".into()],
                app_mode: Some(AppMode::Blacklist),
                ..AppConfig::default()
            },
            rules: vec![RawRule {
                id: "cat-to-dog".into(),
                from: Some("cat".into()),
                to: Some("dog".into()),
                ..RawRule::default()
            }],
        };
        let mut agent = Agent::new(clipboard, notifications, config).unwrap();
        let content = ClipboardItem::from_text("cat").with_source_app(ClipboardSourceApp::new(
            Some("com.example.Ignored".into()),
            Some("Ignored".into()),
        ));

        assert!(agent.handle_clipboard_change(content).unwrap().is_none());
        assert!(agent.clipboard.read().unwrap().is_none());
        assert!(agent.notifications.delivered.is_empty());
        assert!(agent.notifications.startups.is_empty());
    }

    #[test]
    fn source_app_filter_runs_before_payload_read() {
        let payload_reads = std::rc::Rc::new(std::cell::Cell::new(0));
        let clipboard = MetadataFilteredClipboard {
            source_app: ClipboardSourceApp::new(
                Some("com.example.Ignored".into()),
                Some("Ignored".into()),
            ),
            payload_reads: payload_reads.clone(),
        };
        let mut document = config();
        document.config.apps = vec!["com.example.Ignored".into()];
        document.config.app_mode = Some(AppMode::Blacklist);
        let mut agent =
            Agent::new(clipboard, NoopNotificationBackend::default(), document).unwrap();

        assert!(agent.read_clipboard_observation(None).unwrap().is_none());
        assert_eq!(payload_reads.get(), 0);
    }

    #[test]
    fn global_app_whitelist_skips_other_source_apps() {
        let clipboard = MemoryClipboardBackend::new(None);
        let notifications = NoopNotificationBackend::default();
        let config = ConfigDocument {
            plugins: Default::default(),
            config: AppConfig {
                apps: vec!["Allowed".into()],
                app_mode: Some(AppMode::Whitelist),
                ..AppConfig::default()
            },
            rules: vec![RawRule {
                id: "cat-to-dog".into(),
                from: Some("cat".into()),
                to: Some("dog".into()),
                ..RawRule::default()
            }],
        };
        let mut agent = Agent::new(clipboard, notifications, config).unwrap();
        let content = ClipboardItem::from_text("cat").with_source_app(ClipboardSourceApp::new(
            Some("com.example.Other".into()),
            Some("Other".into()),
        ));

        assert!(agent.handle_clipboard_change(content).unwrap().is_none());
        assert!(agent.clipboard.read().unwrap().is_none());
        assert!(agent.notifications.delivered.is_empty());
    }

    #[test]
    fn global_app_filter_requires_mode() {
        let clipboard = MemoryClipboardBackend::new(None);
        let notifications = NoopNotificationBackend::default();
        let config = ConfigDocument {
            plugins: Default::default(),
            config: AppConfig {
                apps: vec!["com.example.App".into()],
                ..AppConfig::default()
            },
            rules: vec![RawRule {
                id: "cat-to-dog".into(),
                from: Some("cat".into()),
                to: Some("dog".into()),
                ..RawRule::default()
            }],
        };

        let err = match Agent::new(clipboard, notifications, config) {
            Ok(_) => panic!("expected global app filter validation error"),
            Err(error) => error,
        };
        assert!(err.to_string().contains("app_mode"));
    }

    #[test]
    fn new_external_change_removes_stale_undo_notification() {
        let clipboard = MemoryClipboardBackend::new(None);
        let notifications = NoopNotificationBackend::default();
        let mut agent = Agent::new(clipboard, notifications, config()).unwrap();
        agent.set_edit_config_path(PathBuf::from("/tmp/config.yaml"));

        agent
            .handle_clipboard_change(ClipboardItem::from_text("cat"))
            .unwrap();
        agent
            .handle_clipboard_change(ClipboardItem::from_text("bird"))
            .unwrap();

        assert_eq!(agent.notifications.removed.len(), 1);
    }

    #[test]
    fn multiple_applied_rules_are_listed_in_notification_body() {
        let clipboard = MemoryClipboardBackend::new(Some(ClipboardItem::from_text("cat")));
        let notifications = NoopNotificationBackend::default();
        let config = ConfigDocument {
            plugins: Default::default(),
            config: AppConfig::default(),
            rules: vec![
                RawRule {
                    id: "cat-to-dog".into(),
                    name: Some("Cat to dog".into()),
                    from: Some("cat".into()),
                    to: Some("dog".into()),
                    ..RawRule::default()
                },
                RawRule {
                    id: "dog-to-wolf".into(),
                    name: Some("Dog to wolf".into()),
                    from: Some("dog".into()),
                    to: Some("wolf".into()),
                    ..RawRule::default()
                },
            ],
        };
        let mut agent = Agent::new(clipboard, notifications, config).unwrap();

        assert!(agent.run_once_from_clipboard().unwrap().is_some());
        assert_eq!(
            agent.notifications.delivered[0].title,
            "2 rules have been applied"
        );
        assert_eq!(
            agent.notifications.delivered[0].body,
            "Cat to dog, Dog to wolf"
        );
    }

    #[test]
    fn startup_notification_reports_rule_count() {
        let clipboard = MemoryClipboardBackend::new(None);
        let notifications = NoopNotificationBackend::default();
        let mut agent = Agent::new(clipboard, notifications, config()).unwrap();

        agent
            .deliver_startup_notification(std::path::Path::new("/tmp/config.yaml"), 1)
            .unwrap();

        assert_eq!(agent.notifications.startups.len(), 1);
        assert_eq!(
            agent.notifications.startups[0].title,
            "Clipboard Transformer is running"
        );
        assert!(agent.notifications.startups[0]
            .body
            .contains("1 valid rule active"));
        assert_eq!(
            agent.notifications.startups[0]
                .edit_target
                .as_ref()
                .map(|target| target.path.as_str()),
            Some("/tmp/config.yaml")
        );
    }

    #[test]
    fn own_clipboard_write_is_not_reprocessed() {
        let clipboard = MemoryClipboardBackend::new(None);
        let notifications = NoopNotificationBackend::default();
        let config = ConfigDocument {
            plugins: Default::default(),
            config: AppConfig::default(),
            rules: vec![RawRule {
                id: "trim-protocol-example".into(),
                from: Some("^https?://example\\.com".into()),
                to: Some("https://example.com".into()),
                ..RawRule::default()
            }],
        };
        let mut agent = Agent::new(clipboard, notifications, config).unwrap();

        assert!(agent
            .handle_clipboard_change(ClipboardItem::from_text("http://example.com/page"))
            .unwrap()
            .is_some());
        assert_eq!(
            agent
                .clipboard
                .read()
                .unwrap()
                .and_then(|content| content.text().map(str::to_string)),
            Some("https://example.com/page".to_string())
        );

        let transformed = agent.clipboard.read().unwrap().unwrap();
        assert!(agent
            .handle_clipboard_change(transformed)
            .unwrap()
            .is_none());
        assert_eq!(agent.notifications.delivered.len(), 1);
    }

    #[test]
    fn double_copy_within_timeout_keeps_original_clipboard() {
        let clipboard = MemoryClipboardBackend::new(None);
        let notifications = NoopNotificationBackend::default();
        let mut agent = Agent::new(clipboard, notifications, config()).unwrap();
        agent.set_edit_config_path(PathBuf::from("/tmp/config.yaml"));

        assert!(agent
            .handle_clipboard_change(ClipboardItem::from_text("cat"))
            .unwrap()
            .is_some());

        let original = ClipboardItem::from_text("cat");
        agent.clipboard.write(&original).unwrap();
        assert!(agent.handle_clipboard_change(original).unwrap().is_none());

        assert_eq!(
            agent
                .clipboard
                .read()
                .unwrap()
                .and_then(|content| content.text().map(str::to_string)),
            Some("cat".to_string())
        );
        assert_eq!(agent.notifications.delivered.len(), 1);
        assert_eq!(agent.notifications.removed.len(), 1);
        assert_eq!(agent.notifications.startups.len(), 1);
        assert_eq!(agent.notifications.startups[0].title, "Rules skipped");
        let body = &agent.notifications.startups[0].body;
        assert!(body.starts_with("Copied again within 10 "));
        assert!(body.ends_with("; kept original."));
        assert_eq!(
            agent.notifications.startups[0]
                .edit_target
                .as_ref()
                .map(|target| target.path.as_str()),
            Some("/tmp/config.yaml")
        );
    }

    #[test]
    fn double_copy_after_timeout_is_transformed_again() {
        let clipboard = MemoryClipboardBackend::new(None);
        let notifications = NoopNotificationBackend::default();
        let mut agent = Agent::new(clipboard, notifications, config()).unwrap();

        assert!(agent
            .handle_clipboard_change(ClipboardItem::from_text("cat"))
            .unwrap()
            .is_some());
        agent.last_transformed_external = agent.last_transformed_external.map(|mut recent| {
            recent.transformed_at =
                Instant::now() - Duration::from_secs(AppConfig::default().double_copy_window + 1);
            recent
        });

        let original = ClipboardItem::from_text("cat");
        agent.clipboard.write(&original).unwrap();
        assert!(agent.handle_clipboard_change(original).unwrap().is_some());

        assert_eq!(
            agent
                .clipboard
                .read()
                .unwrap()
                .and_then(|content| content.text().map(str::to_string)),
            Some("dog".to_string())
        );
        assert_eq!(agent.notifications.delivered.len(), 2);
    }

    #[test]
    fn undo_command_restores_clipboard_and_removes_notification() {
        let clipboard = MemoryClipboardBackend::new(None);
        let notifications = NoopNotificationBackend::default();
        let mut agent = Agent::new(clipboard, notifications, config()).unwrap();
        let transform_id = agent
            .handle_clipboard_change(ClipboardItem::from_text("cat"))
            .unwrap()
            .unwrap();

        agent
            .handle_event(AppEvent::UserCommand(AppCommand::Undo { transform_id }))
            .unwrap();

        assert_eq!(agent.clipboard.read().unwrap().unwrap().text(), Some("cat"));
        assert_eq!(agent.notifications.removed.len(), 1);
    }

    #[test]
    fn stale_undo_command_removes_notification() {
        let clipboard = MemoryClipboardBackend::new(None);
        let notifications = NoopNotificationBackend::default();
        let mut agent = Agent::new(clipboard, notifications, config()).unwrap();
        let transform_id = agent
            .handle_clipboard_change(ClipboardItem::from_text("cat"))
            .unwrap()
            .unwrap();
        agent
            .clipboard
            .write(&ClipboardItem::from_text("unrelated"))
            .unwrap();

        agent
            .handle_event(AppEvent::UserCommand(AppCommand::Undo { transform_id }))
            .unwrap();

        assert_eq!(
            agent.notifications.removed,
            vec![format!("clipboard-transformer-{transform_id}")]
        );
    }

    #[test]
    fn history_restore_replaces_unrelated_clipboard_content() {
        let clipboard = MemoryClipboardBackend::new(None);
        let notifications = NoopNotificationBackend::default();
        let mut agent = Agent::new(clipboard, notifications, config()).unwrap();
        let transform_id = agent
            .handle_clipboard_change(ClipboardItem::from_text("cat"))
            .unwrap()
            .unwrap();
        agent
            .clipboard
            .write(&ClipboardItem::from_text("unrelated"))
            .unwrap();
        agent
            .handle_event(AppEvent::UserCommand(AppCommand::SetPaused(true)))
            .unwrap();

        agent
            .handle_event(AppEvent::UserCommand(AppCommand::RestoreHistory {
                transform_id,
            }))
            .unwrap();

        assert_eq!(agent.clipboard.read().unwrap().unwrap().text(), Some("cat"));
        assert_eq!(agent.notifications.removed.len(), 1);
    }

    #[test]
    fn reload_warnings_are_exposed_in_the_tray_snapshot() {
        let mut agent = Agent::new(
            MemoryClipboardBackend::new(None),
            NoopNotificationBackend::default(),
            config(),
        )
        .unwrap();
        let document = config();
        let engine = RuleEngine::compile(document.rules.clone()).unwrap();
        let warning = ConfigWarning::IgnoredRuleType {
            kind: "plugin".into(),
        };

        agent
            .handle_event(AppEvent::ConfigReloaded(reload::ReloadOutcome::Applied {
                config: Box::new(document),
                engine,
                rule_count: 1,
                watched_sources: [PathBuf::from("/tmp/config.yaml")].into(),
                rule_sources: BTreeMap::new(),
                warnings: vec![warning.clone()],
                plugin_statuses: Vec::new(),
                plugin_fingerprint: 0,
            }))
            .unwrap();

        assert_eq!(agent.tray_snapshot().config_warnings, vec![warning]);
    }

    #[test]
    fn reload_reconfigures_notification_disable_action() {
        let mut agent = Agent::new(
            MemoryClipboardBackend::new(None),
            NoopNotificationBackend::default(),
            config(),
        )
        .unwrap();
        let mut document = config();
        document.config.disable_for = 30;
        let engine = RuleEngine::compile(document.rules.clone()).unwrap();

        agent
            .handle_event(AppEvent::ConfigReloaded(reload::ReloadOutcome::Applied {
                config: Box::new(document),
                engine,
                rule_count: 1,
                watched_sources: [PathBuf::from("/tmp/config.yaml")].into(),
                rule_sources: BTreeMap::new(),
                warnings: Vec::new(),
                plugin_statuses: Vec::new(),
                plugin_fingerprint: 0,
            }))
            .unwrap();

        assert_eq!(agent.notifications.configured_disable_for, [30]);
    }

    #[test]
    fn edit_and_reload_commands_return_host_effects() {
        let clipboard = MemoryClipboardBackend::new(None);
        let notifications = NoopNotificationBackend::default();
        let mut document = config();
        document.config.editor = Some(EditorConfig {
            command: "code".into(),
            args: vec!["--goto".into(), "{file}:{line}:{column}".into()],
        });
        let mut agent = Agent::new(clipboard, notifications, document).unwrap();
        agent.set_edit_config_path(PathBuf::from("/tmp/config.yaml"));
        agent.set_rule_sources(BTreeMap::from([(
            "rule".to_string(),
            RuleSource {
                path: PathBuf::from("/tmp/imported.yaml"),
                line: 12,
            },
        )]));

        let edit = agent
            .handle_event(AppEvent::UserCommand(AppCommand::EditRule {
                rule_id: Some("rule".into()),
            }))
            .unwrap();
        let reload = agent
            .handle_event(AppEvent::UserCommand(AppCommand::ReloadConfig))
            .unwrap();

        assert_eq!(
            edit,
            vec![AppEffect::OpenEditor {
                target: EditTarget {
                    path: "/tmp/imported.yaml".into(),
                    line: Some(12),
                },
                editor: Some(EditorConfig {
                    command: "code".into(),
                    args: vec!["--goto".into(), "{file}:{line}:{column}".into()],
                }),
            }]
        );
        assert_eq!(reload, vec![AppEffect::ReloadConfig]);
    }

    #[test]
    fn disable_command_prevents_matching_rule_temporarily() {
        let clipboard = MemoryClipboardBackend::new(None);
        let notifications = NoopNotificationBackend::default();
        let mut agent = Agent::new(clipboard, notifications, config()).unwrap();

        agent
            .handle_event(AppEvent::UserCommand(AppCommand::DisableRule {
                rule_id: "rule".into(),
                seconds: 60,
            }))
            .unwrap();

        assert!(agent
            .handle_clipboard_change(ClipboardItem::from_text("cat"))
            .unwrap()
            .is_none());
        assert!(agent.notifications.delivered.is_empty());
    }

    #[test]
    fn tray_snapshot_contains_recent_transform_and_config_status() {
        let clipboard = MemoryClipboardBackend::new(None);
        let notifications = NoopNotificationBackend::default();
        let mut agent = Agent::new(clipboard, notifications, config()).unwrap();
        agent.set_edit_config_path(PathBuf::from("/tmp/config.yaml"));
        agent.set_tray_source_count(3);

        let transform_id = agent
            .handle_clipboard_change(ClipboardItem::from_text("cat"))
            .unwrap()
            .unwrap();
        let snapshot = agent.tray_snapshot();

        assert_eq!(snapshot.rule_count, 1);
        assert_eq!(snapshot.source_count, 3);
        assert_eq!(
            snapshot.config_path,
            Some(PathBuf::from("/tmp/config.yaml"))
        );
        assert_eq!(snapshot.recent.len(), 1);
        assert_eq!(snapshot.recent[0].transform_id, transform_id);
        assert_eq!(snapshot.recent[0].result, "dog");
        assert_eq!(snapshot.recent[0].rules[0].id, "rule");
        assert!(snapshot.recent[0].can_undo);
    }

    #[test]
    fn zero_recent_items_count_disables_history() {
        let mut document = config();
        document.config.recent_items_count = 0;
        let mut agent = Agent::new(
            MemoryClipboardBackend::new(None),
            NoopNotificationBackend::default(),
            document,
        )
        .unwrap();

        agent
            .handle_clipboard_change(ClipboardItem::from_text("cat"))
            .unwrap();

        assert!(agent.tray_snapshot().recent.is_empty());
    }

    #[test]
    fn oversized_clipboard_item_is_ignored() {
        let mut document = config();
        document.config.max_item_bytes = 2;
        let mut agent = Agent::new(
            MemoryClipboardBackend::new(None),
            NoopNotificationBackend::default(),
            document,
        )
        .unwrap();

        assert!(agent
            .handle_clipboard_change(ClipboardItem::from_text("cat"))
            .unwrap()
            .is_none());
        assert!(agent.tray_snapshot().recent.is_empty());
    }

    #[test]
    fn total_history_limit_prunes_oldest_records() {
        let mut document = config();
        document.config.max_history_bytes = 6;
        let mut agent = Agent::new(
            MemoryClipboardBackend::new(None),
            NoopNotificationBackend::default(),
            document,
        )
        .unwrap();
        let newest_id = Uuid::new_v4();
        let oldest_id = Uuid::new_v4();
        agent.history_index = [(newest_id, 6), (oldest_id, 6)].into();
        for transform_id in [oldest_id, newest_id] {
            agent.undo.remember(LastTransform {
                transform_id,
                rule_id: Some("rule".into()),
                previous: ClipboardItem::from_text("before"),
                transformed: ClipboardItem::from_text("after"),
                notification_id: format!("clipboard-transformer-{transform_id}"),
            });
        }

        agent.prune_persistent_history();

        assert_eq!(agent.history_index.len(), 1);
        assert_eq!(agent.history_index[0], (newest_id, 6));
        assert_eq!(
            agent.notifications.removed,
            vec![format!("clipboard-transformer-{oldest_id}")]
        );
    }

    #[test]
    fn transform_is_skipped_when_its_history_entry_cannot_fit() {
        let mut document = config();
        document.config.max_history_bytes = 1;
        let mut agent = Agent::new(
            MemoryClipboardBackend::new(Some(ClipboardItem::from_text("cat"))),
            NoopNotificationBackend::default(),
            document,
        )
        .unwrap();

        let transform_id = agent
            .handle_clipboard_change(ClipboardItem::from_text("cat"))
            .unwrap();

        assert!(transform_id.is_none());
        assert_eq!(agent.clipboard.read().unwrap().unwrap().text(), Some("cat"));
        assert!(agent.notifications.delivered.is_empty());
        assert!(agent.undo.notification_ids().next().is_none());
        assert!(agent.history_index.is_empty());
    }

    #[test]
    fn pause_toggle_skips_rules_until_resumed() {
        let mut agent = Agent::new(
            MemoryClipboardBackend::new(None),
            NoopNotificationBackend::default(),
            config(),
        )
        .unwrap();

        agent
            .handle_event(AppEvent::UserCommand(AppCommand::SetPaused(true)))
            .unwrap();
        assert!(agent
            .handle_clipboard_change(ClipboardItem::from_text("cat"))
            .unwrap()
            .is_none());

        agent
            .handle_event(AppEvent::UserCommand(AppCommand::SetPaused(false)))
            .unwrap();
        assert!(agent
            .handle_clipboard_change(ClipboardItem::from_text("cat"))
            .unwrap()
            .is_some());
    }

    #[test]
    fn config_path_commands_use_host_effects_and_clipboard_port() {
        let mut agent = Agent::new(
            MemoryClipboardBackend::new(None),
            NoopNotificationBackend::default(),
            config(),
        )
        .unwrap();
        let path = PathBuf::from("/tmp/config.yaml");
        agent.set_edit_config_path(path.clone());

        assert_eq!(
            agent
                .handle_event(AppEvent::UserCommand(AppCommand::OpenConfig))
                .unwrap(),
            vec![AppEffect::OpenConfig(path.clone())]
        );
        assert_eq!(
            agent
                .handle_event(AppEvent::UserCommand(AppCommand::RevealConfig))
                .unwrap(),
            vec![AppEffect::RevealConfig(path)]
        );
        agent
            .handle_event(AppEvent::UserCommand(AppCommand::CopyConfigPath))
            .unwrap();
        assert_eq!(
            agent.clipboard.read().unwrap().unwrap().text(),
            Some("/tmp/config.yaml")
        );
    }

    #[test]
    fn history_and_undo_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state");
        let mut first = Agent::new(
            MemoryClipboardBackend::new(None),
            NoopNotificationBackend::default(),
            config(),
        )
        .unwrap();
        first.load_persistent_state(state_dir.clone()).unwrap();
        let transform_id = first
            .handle_clipboard_change(ClipboardItem::from_text("cat"))
            .unwrap()
            .unwrap();
        first.disable_rule("rule".into(), 600);
        first
            .handle_event(AppEvent::UserCommand(AppCommand::SetPaused(true)))
            .unwrap();
        drop(first);

        let mut restored = Agent::new(
            SequencedClipboard::new(Some(ClipboardItem::from_text("dog"))),
            NoopNotificationBackend::default(),
            config(),
        )
        .unwrap();
        restored.load_persistent_state(state_dir).unwrap();

        assert_eq!(restored.tray_snapshot().recent.len(), 1);
        assert!(restored.tray_snapshot().paused);
        assert!(restored.disabled_rules.contains_key("rule"));
        assert!(restored.tray_snapshot().recent[0].can_undo);

        let mut last_change_count = None;
        assert!(restored
            .poll_clipboard(&mut last_change_count)
            .unwrap()
            .is_none());
        assert!(restored.tray_snapshot().recent[0].can_undo);

        restored
            .handle_event(AppEvent::UserCommand(AppCommand::Undo { transform_id }))
            .unwrap();
        assert_eq!(
            restored.clipboard.read().unwrap().unwrap().text(),
            Some("cat")
        );
    }

    #[test]
    fn clear_history_removes_memory_and_persisted_history() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state");
        let mut agent = Agent::new(
            MemoryClipboardBackend::new(None),
            NoopNotificationBackend::default(),
            config(),
        )
        .unwrap();
        agent.load_persistent_state(state_dir.clone()).unwrap();
        let transform_id = agent
            .handle_clipboard_change(ClipboardItem::from_text("cat"))
            .unwrap()
            .unwrap();

        agent
            .handle_event(AppEvent::UserCommand(AppCommand::ClearHistory))
            .unwrap();
        agent.flush_history_writer();

        assert!(agent.tray_snapshot().recent.is_empty());
        assert!(PersistentHistory::load(&state_dir.join("history.cbor"))
            .unwrap()
            .items
            .is_empty());
        assert_eq!(
            agent.notifications.removed,
            vec![format!("clipboard-transformer-{transform_id}")]
        );
    }

    #[test]
    fn latest_external_clipboard_is_saved_even_without_a_transform() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state");
        let mut document = config();
        document.config.persist_last_clipboard = true;
        let mut agent = Agent::new(
            MemoryClipboardBackend::new(None),
            NoopNotificationBackend::default(),
            document,
        )
        .unwrap();
        agent.load_persistent_state(state_dir.clone()).unwrap();

        assert!(agent
            .handle_clipboard_change(ClipboardItem::from_text("no rule matches this"))
            .unwrap()
            .is_none());

        let snapshot = LastClipboardSnapshot::load(&state_dir.join("last-clipboard.cbor"))
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.item.text(), Some("no rule matches this"));
        assert!(snapshot.observed_at_unix_ms > 0);
    }

    #[test]
    fn last_clipboard_persistence_is_opt_in_and_uses_selective_reads_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(state_dir.join("last-clipboard.cbor"), b"stale").unwrap();
        let mut agent = Agent::new(
            SelectiveClipboard::default(),
            NoopNotificationBackend::default(),
            config(),
        )
        .unwrap();
        agent.load_persistent_state(state_dir.clone()).unwrap();

        agent.run_once_from_clipboard().unwrap();

        assert_eq!(agent.clipboard.full_reads, 0);
        assert_eq!(agent.clipboard.selective_reads, 1);
        assert!(!state_dir.join("last-clipboard.cbor").exists());
    }

    #[test]
    fn corrupt_persistent_files_are_recovered_without_blocking_startup() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(state_dir.join("state.json"), b"not-json").unwrap();
        std::fs::write(state_dir.join("history.cbor"), b"not-cbor").unwrap();
        std::fs::write(state_dir.join("last-clipboard.cbor"), b"not-cbor").unwrap();
        let mut document = config();
        document.config.persist_last_clipboard = true;
        let mut agent = Agent::new(
            MemoryClipboardBackend::new(None),
            NoopNotificationBackend::default(),
            document,
        )
        .unwrap();

        agent.load_persistent_state(state_dir.clone()).unwrap();
        agent.flush_history_writer();

        let names = std::fs::read_dir(&state_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(names
            .iter()
            .any(|name| name.starts_with("state.json.corrupt-")));
        assert!(state_dir.join("history.cbor").is_file());
        assert!(!state_dir.join("last-clipboard.cbor").exists());
        assert!(!names
            .iter()
            .any(|name| name.starts_with("history.cbor.corrupt-")));
        assert!(!names
            .iter()
            .any(|name| name.starts_with("last-clipboard.cbor.corrupt-")));
    }
}
