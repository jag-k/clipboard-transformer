use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};

use crate::config::{
    collect_config_sources_best_effort_with_options, load_config_with_options, ConfigDocument,
    ConfigLoadOptions, ConfigWarning, RuleSource,
};
use crate::logging;
use crate::platform::environment::EnvironmentRefreshMode;
use crate::plugins::{PluginCatalog, PluginLimits, PluginSet, PluginStatus};
use ct_core::RuleEngine;
use std::collections::BTreeMap;

const RELOAD_DEBOUNCE: Duration = Duration::from_millis(250);
const RELOAD_REQUEST_WATCHDOG: Duration = Duration::from_secs(5);
const RELOAD_REQUEST_FILE: &str = "reload-request";

fn serialized_fingerprint(value: &impl serde::Serialize) -> Result<[u8; 32]> {
    struct DigestWriter<'a>(&'a mut Sha256);

    impl Write for DigestWriter<'_> {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.update(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut digest = Sha256::new();
    serde_json::to_writer(DigestWriter(&mut digest), value).context("serialize fingerprint")?;
    Ok(digest.finalize().into())
}

#[doc(hidden)]
pub fn config_fingerprint(document: &ConfigDocument) -> Result<[u8; 32]> {
    serialized_fingerprint(document).context("fingerprint effective config")
}

#[doc(hidden)]
pub fn load_metadata_fingerprint(
    warnings: &[ConfigWarning],
    rule_sources: &BTreeMap<String, RuleSource>,
) -> Result<[u8; 32]> {
    serialized_fingerprint(&(warnings, rule_sources)).context("fingerprint config load metadata")
}

fn source_contents_fingerprint(
    sources: &BTreeSet<PathBuf>,
    dotenv_path: &Path,
) -> Result<[u8; 32]> {
    let mut digest = Sha256::new();
    for path in sources
        .iter()
        .map(PathBuf::as_path)
        .chain(std::iter::once(dotenv_path))
    {
        let path_bytes = path.as_os_str().as_encoded_bytes();
        digest.update(path_bytes.len().to_le_bytes());
        digest.update(path_bytes);
        match fs::File::open(path) {
            Ok(mut file) => {
                digest.update([1]);
                let mut buffer = [0_u8; 16 * 1024];
                loop {
                    let read = file
                        .read(&mut buffer)
                        .with_context(|| format!("read {}", path.display()))?;
                    if read == 0 {
                        break;
                    }
                    digest.update(&buffer[..read]);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => digest.update([0]),
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        }
    }
    Ok(digest.finalize().into())
}

pub struct ConfigReloader {
    config_path: PathBuf,
    watcher: RecommendedWatcher,
    events: Receiver<notify::Result<Event>>,
    watched_sources: BTreeSet<PathBuf>,
    remote_imports: BTreeMap<String, PathBuf>,
    watched_dirs: BTreeSet<PathBuf>,
    load_options: ConfigLoadOptions,
    reload_request_path: Option<PathBuf>,
    reload_request_watched: bool,
    ignored_watch_dir: Option<PathBuf>,
    dotenv_path: PathBuf,
    plugins_dir: Option<PathBuf>,
    plugins_dir_watched: bool,
    last_document_fingerprint: [u8; 32],
    last_source_contents_fingerprint: [u8; 32],
    import_refresh_interval: u64,
    last_load_metadata_fingerprint: [u8; 32],
    last_plugin_modules: BTreeMap<PathBuf, u64>,
    last_environment_revision: u64,
    last_url_check: Instant,
    last_reload_request_check: Instant,
    pending_since: Option<Instant>,
    pending_force_reload: bool,
    last_error: Option<String>,
    watch_targets: Arc<RwLock<WatchTargets>>,
}

#[derive(Default)]
pub struct ConfigReloaderHost {
    pub plugins_dir: Option<PathBuf>,
    pub wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

pub struct ConfigReloaderState {
    pub watched_sources: BTreeSet<PathBuf>,
    pub remote_imports: BTreeMap<String, PathBuf>,
    pub import_refresh_interval: u64,
    pub document_fingerprint: [u8; 32],
    pub load_metadata_fingerprint: [u8; 32],
}

#[derive(Clone)]
struct WatchTargets {
    sources: BTreeSet<PathBuf>,
    dotenv_path: PathBuf,
    plugins_dir: Option<PathBuf>,
    reload_request_path: Option<PathBuf>,
}

#[derive(Debug)]
pub enum ReloadOutcome {
    Applied {
        config: Box<ConfigDocument>,
        engine: RuleEngine,
        rule_count: usize,
        watched_sources: BTreeSet<PathBuf>,
        rule_sources: BTreeMap<String, RuleSource>,
        warnings: Vec<ConfigWarning>,
        plugin_statuses: Vec<PluginStatus>,
        plugin_fingerprint: u64,
    },
    Unchanged,
    Failed {
        error: String,
        reload_request_path: Option<PathBuf>,
    },
}

impl ConfigReloader {
    pub fn new(
        config_path: impl Into<PathBuf>,
        state: ConfigReloaderState,
        load_options: ConfigLoadOptions,
        host: ConfigReloaderHost,
    ) -> Result<Self> {
        let ConfigReloaderState {
            watched_sources,
            remote_imports,
            import_refresh_interval,
            document_fingerprint: last_document_fingerprint,
            load_metadata_fingerprint: last_load_metadata_fingerprint,
        } = state;
        let ConfigReloaderHost { plugins_dir, wake } = host;
        let config_path = config_path.into();
        let dotenv_path = crate::platform::environment::dotenv_path(&config_path);
        let reload_request_path = load_options
            .state_dir
            .as_ref()
            .map(|state_dir| state_dir.join(RELOAD_REQUEST_FILE));
        let watch_targets = Arc::new(RwLock::new(WatchTargets {
            sources: watched_sources.clone(),
            dotenv_path: dotenv_path.clone(),
            plugins_dir: plugins_dir.clone(),
            reload_request_path: reload_request_path.clone(),
        }));
        let (sender, events) = mpsc::channel();
        let callback_targets = Arc::clone(&watch_targets);
        let mut watcher = RecommendedWatcher::new(
            move |event| {
                let relevant = callback_targets
                    .read()
                    .map(|targets| watch_event_affects_targets(&event, &targets))
                    .unwrap_or(true);
                if !relevant {
                    return;
                }
                if sender.send(event).is_ok() {
                    if let Some(wake) = &wake {
                        wake();
                    }
                }
            },
            Config::default(),
        )?;
        let ignored_watch_dir = load_options
            .state_dir
            .as_ref()
            .map(|state_dir| state_dir.join("url-imports"));
        let watched_dirs = source_dirs(&watched_sources, ignored_watch_dir.as_deref());
        for dir in &watched_dirs {
            watch_dir(&mut watcher, dir)?;
        }
        let mut reload_request_watched = false;
        if let Some(reload_request_dir) = reload_request_path.as_deref().and_then(Path::parent) {
            if watched_dirs.contains(reload_request_dir) {
                reload_request_watched = true;
            } else {
                match watch_dir(&mut watcher, reload_request_dir) {
                    Ok(()) => {
                        reload_request_watched = true;
                        logging::event(format!(
                            "config reloader watching manual request dir {}",
                            reload_request_dir.display()
                        ));
                    }
                    Err(error) => logging::event(format!(
                        "manual reload request watch unavailable: {error:#}"
                    )),
                }
            }
        }
        let mut plugins_dir_watched = false;
        let mut last_plugin_modules = BTreeMap::new();
        if let Some(dir) = &plugins_dir {
            last_plugin_modules = PluginCatalog::discover(dir).module_fingerprints();
            match watch_dir(&mut watcher, dir) {
                Ok(()) => plugins_dir_watched = true,
                // A missing plugins directory degrades plugin hot reload; a
                // later successful reload retries the watch.
                Err(error) => logging::event(format!("plugins watch unavailable: {error:#}")),
            }
        }
        logging::event(format!(
            "config reloader watching {} source(s) in {} dir(s), plugins_dir_watched={plugins_dir_watched}",
            watched_sources.len(),
            watched_dirs.len()
        ));
        let last_source_contents_fingerprint =
            source_contents_fingerprint(&watched_sources, &dotenv_path)?;
        Ok(Self {
            config_path,
            watcher,
            events,
            watched_sources,
            remote_imports,
            watched_dirs,
            load_options,
            reload_request_path,
            reload_request_watched,
            ignored_watch_dir,
            dotenv_path,
            plugins_dir,
            plugins_dir_watched,
            import_refresh_interval,
            last_document_fingerprint,
            last_source_contents_fingerprint,
            last_load_metadata_fingerprint,
            last_plugin_modules,
            last_environment_revision: crate::platform::environment::revision(),
            last_url_check: Instant::now(),
            last_reload_request_check: Instant::now(),
            pending_since: None,
            pending_force_reload: false,
            last_error: None,
            watch_targets,
        })
    }

    pub fn poll(&mut self) -> Result<Option<ReloadOutcome>> {
        self.last_reload_request_check = Instant::now();
        let mut saw_event = false;
        let mut force_reload = false;
        for event in self.events.try_iter() {
            match event {
                Ok(event) => {
                    if self.event_affects_watched_sources(&event) {
                        saw_event = true;
                        force_reload |= event.paths.is_empty()
                            || event
                                .paths
                                .iter()
                                .any(|path| self.event_affects_plugins(path));
                        logging::event(format!("config file event kind={:?}", event.kind));
                    } else {
                        logging::event(format!("config file event ignored kind={:?}", event.kind));
                    }
                }
                Err(error) => logging::event(format!("config watcher error: {error}")),
            }
        }
        // Refresh the debounce window on every event so a save sequence
        // longer than the debounce doesn't trigger a reload mid-write.
        if saw_event {
            self.pending_since = Some(Instant::now());
            self.pending_force_reload |= force_reload;
        }

        if self.consume_reload_request() {
            self.pending_since = None;
            self.pending_force_reload = false;
            return self.reload_now(true);
        }

        let Some(pending_since) = self.pending_since else {
            if self.url_refresh_due() {
                self.last_url_check = Instant::now();
                match crate::config::refresh_remote_imports(&self.remote_imports) {
                    Ok(false) => {
                        self.last_error = None;
                        logging::event("URL imports checked; no changes found");
                        return Ok(Some(ReloadOutcome::Unchanged));
                    }
                    Ok(true) | Err(_) => return self.reload_now(false),
                }
            }
            return Ok(None);
        };
        if pending_since.elapsed() < RELOAD_DEBOUNCE {
            return Ok(None);
        }

        self.pending_since = None;
        let force_reload = std::mem::take(&mut self.pending_force_reload);
        if !force_reload {
            match source_contents_fingerprint(&self.watched_sources, &self.dotenv_path) {
                Ok(fingerprint) if fingerprint == self.last_source_contents_fingerprint => {
                    logging::event(
                        "config file event ignored because watched contents are unchanged",
                    );
                    return Ok(None);
                }
                Ok(_) => {}
                Err(error) => logging::event(format!(
                    "config source fingerprint unavailable; attempting reload: {error:#}"
                )),
            }
        }
        self.reload_now(false)
    }

    pub fn reload(&mut self) -> Result<Option<ReloadOutcome>> {
        self.pending_since = None;
        self.pending_force_reload = false;
        self.reload_now(true)
    }

    /// Earliest time at which polling this reloader can produce new work
    /// without a filesystem notification.
    pub fn next_deadline(&self) -> Option<Instant> {
        let debounce = self
            .pending_since
            .and_then(|pending| pending.checked_add(RELOAD_DEBOUNCE));
        let interval = Duration::from_secs(self.import_refresh_interval);
        let url_refresh = (!self.remote_imports.is_empty()
            && !interval.is_zero()
            && self.load_options.refresh_url_imports)
            .then(|| self.last_url_check.checked_add(interval))
            .flatten();
        let request_watchdog = (self.reload_request_path.is_some() && !self.reload_request_watched)
            .then(|| {
                self.last_reload_request_check
                    .checked_add(RELOAD_REQUEST_WATCHDOG)
            })
            .flatten();
        [debounce, url_refresh, request_watchdog]
            .into_iter()
            .flatten()
            .min()
    }

    fn reload_now(&mut self, requested: bool) -> Result<Option<ReloadOutcome>> {
        self.last_url_check = Instant::now();
        match self.try_reload(requested) {
            Ok(outcome) => {
                self.last_error = None;
                Ok(Some(outcome))
            }
            Err(error) => {
                let error = format!("{error:#}");
                let sources = collect_config_sources_best_effort_with_options(
                    &self.config_path,
                    ConfigLoadOptions {
                        refresh_url_imports: false,
                        ..self.load_options.clone()
                    },
                );
                if !sources.is_empty() {
                    self.replace_watches(&sources);
                }
                // Background polls dedupe repeated failures, but a reload the
                // user explicitly requested always reports its outcome.
                if !requested && self.last_error.as_deref() == Some(error.as_str()) {
                    logging::event(format!("config reload still failing: {error}"));
                    Ok(None)
                } else {
                    self.last_error = Some(error.clone());
                    Ok(Some(ReloadOutcome::Failed {
                        error,
                        reload_request_path: self.reload_request_path.clone(),
                    }))
                }
            }
        }
    }

    fn try_reload(&mut self, requested: bool) -> Result<ReloadOutcome> {
        let mode = if requested {
            EnvironmentRefreshMode::Explicit
        } else {
            EnvironmentRefreshMode::Background
        };
        let environment = crate::platform::environment::refresh_for_config(&self.config_path, mode);
        if let Some(warning) = environment.shell_warning {
            logging::event(format!(
                "GUI login-shell environment refresh failed; keeping previous snapshot: {warning}"
            ));
        }
        if environment.dotenv_changed {
            logging::event(format!(
                "dotenv reloaded path={} present={} loaded={} ignored={}",
                environment.dotenv.path.display(),
                environment.dotenv.present,
                environment.dotenv.loaded_count,
                environment.dotenv.ignored_count
            ));
        }
        let environment_revision = environment.revision;
        let environment_changed = environment_revision != self.last_environment_revision;
        // Plugins reload together with the config: their manifests define
        // which rule types are valid and their permissions come from the
        // config, so one combined rebuild keeps both consistent.
        let (catalog, plugin_modules) = match &self.plugins_dir {
            Some(dir) => {
                let catalog = PluginCatalog::discover(dir);
                let fingerprints = catalog.module_fingerprints();
                (catalog, fingerprints)
            }
            None => (
                PluginCatalog {
                    entries: Vec::new(),
                },
                BTreeMap::new(),
            ),
        };
        self.load_options.known_rule_types = catalog.known_rule_types();
        let mut loaded = load_config_with_options(&self.config_path, self.load_options.clone())?;
        let document_fingerprint = config_fingerprint(&loaded.document)?;
        let load_metadata_fingerprint =
            load_metadata_fingerprint(&loaded.warnings, &loaded.rule_sources)?;
        loaded
            .document
            .config
            .app_matcher()
            .context("compile reloaded config app filter")?;
        if document_fingerprint == self.last_document_fingerprint
            && loaded.sources == self.watched_sources
            && load_metadata_fingerprint == self.last_load_metadata_fingerprint
            && plugin_modules == self.last_plugin_modules
            && !environment_changed
        {
            logging::event("config reload checked; no changes found");
            return Ok(ReloadOutcome::Unchanged);
        }
        // The effective schema tracks the discovered provider set; refresh it
        // best-effort so editors pick up added or removed plugin rule types.
        let schema_contributions = crate::config::plugin_schema_contributions(&catalog);
        match crate::config::json_schema_pretty_with_plugins(&schema_contributions) {
            Ok(schema) => {
                if let Err(error) =
                    crate::config::sync_config_schema_contents_next_to(&self.config_path, &schema)
                {
                    logging::event(format!("runtime schema refresh failed: {error:#}"));
                }
            }
            Err(error) => logging::event(format!("runtime schema generation failed: {error:#}")),
        }
        let plugin_set: PluginSet = catalog.initialize_for_rules(
            &loaded.document.plugins,
            &PluginLimits::default(),
            &loaded.document.rules,
        );
        let rules = std::mem::take(&mut loaded.document.rules);
        let mut resolved_paths = crate::config::ConfigPaths::resolve()?;
        if let Some(state_dir) = &self.load_options.state_dir {
            resolved_paths.state_dir = state_dir.clone();
        }
        let shell_paths = crate::rules::shell::ShellHostPaths::from_config_paths(
            &resolved_paths,
            Some(self.config_path.clone()),
        );
        let external_providers = crate::rules::external_providers(
            &loaded.document.config,
            shell_paths,
            &loaded.rule_sources,
            plugin_set.providers(),
        );
        let (engine, skipped) = RuleEngine::compile_with_external(rules, &external_providers)
            .context("compile reloaded config rules")?;
        let mut warnings = loaded.warnings;
        warnings.extend(
            skipped
                .into_iter()
                .map(|skipped| ConfigWarning::InvalidRule {
                    id: Some(skipped.id),
                    kind: skipped.kind,
                    reason: skipped.reason,
                }),
        );
        let rule_count = engine.rule_count();
        self.replace_watches(&loaded.sources);
        self.retry_plugins_watch();
        self.remote_imports = loaded.remote_imports.clone();
        self.import_refresh_interval = loaded.document.config.import_refresh_interval;
        self.last_document_fingerprint = document_fingerprint;
        self.last_source_contents_fingerprint =
            source_contents_fingerprint(&loaded.sources, &self.dotenv_path)?;
        self.last_load_metadata_fingerprint = load_metadata_fingerprint;
        self.last_plugin_modules = plugin_modules;
        self.last_environment_revision = environment_revision;
        logging::event(format!(
            "config reload applied rules={rule_count} sources={} plugins={}",
            loaded.sources.len(),
            plugin_set.statuses().len()
        ));
        let plugin_fingerprint = plugin_set.issue_fingerprint();
        Ok(ReloadOutcome::Applied {
            config: Box::new(loaded.document),
            engine,
            rule_count,
            watched_sources: loaded.sources,
            rule_sources: loaded.rule_sources,
            warnings,
            plugin_statuses: plugin_set.into_statuses(),
            plugin_fingerprint,
        })
    }

    fn retry_plugins_watch(&mut self) {
        if self.plugins_dir_watched {
            return;
        }
        let Some(dir) = &self.plugins_dir else {
            return;
        };
        if watch_dir(&mut self.watcher, dir).is_ok() {
            self.plugins_dir_watched = true;
            logging::event(format!("plugins watch established for {}", dir.display()));
        }
    }

    fn url_refresh_due(&self) -> bool {
        let interval = Duration::from_secs(self.import_refresh_interval);
        !self.remote_imports.is_empty()
            && !interval.is_zero()
            && self.load_options.refresh_url_imports
            && self.last_url_check.elapsed() >= interval
    }

    // A directory that fails to watch (e.g. it was just deleted) degrades hot
    // reload for that directory instead of taking the whole app down.
    fn replace_watches(&mut self, next_sources: &BTreeSet<PathBuf>) {
        if let Ok(mut targets) = self.watch_targets.write() {
            targets.sources = next_sources.clone();
        }
        let next_dirs = source_dirs(next_sources, self.ignored_watch_dir.as_deref());
        for dir in self.watched_dirs.difference(&next_dirs) {
            if let Err(error) = self.watcher.unwatch(dir) {
                logging::event(format!(
                    "config watcher unwatch failed {}: {error}",
                    dir.display()
                ));
            }
        }
        let mut watched = BTreeSet::new();
        for dir in &next_dirs {
            if self.watched_dirs.contains(dir) {
                watched.insert(dir.clone());
                continue;
            }
            match watch_dir(&mut self.watcher, dir) {
                Ok(()) => {
                    watched.insert(dir.clone());
                }
                Err(error) => logging::event(format!("config watch failed: {error:#}")),
            }
        }
        self.watched_sources = next_sources.clone();
        self.watched_dirs = watched;
    }

    fn event_affects_watched_sources(&self, event: &Event) -> bool {
        event.paths.is_empty()
            || event.paths.iter().any(|path| {
                path_affects_config(path, &self.watched_sources, &self.dotenv_path)
                    || self.event_affects_plugins(path)
                    || self
                        .reload_request_path
                        .as_deref()
                        .is_some_and(|request| paths_equivalent(path, request))
            })
    }

    fn event_affects_plugins(&self, path: &Path) -> bool {
        path_affects_plugins(path, self.plugins_dir.as_deref())
    }

    fn consume_reload_request(&self) -> bool {
        let Some(path) = &self.reload_request_path else {
            return false;
        };
        match fs::remove_file(path) {
            Ok(()) => {
                logging::event(format!(
                    "manual config reload requested path={}",
                    path.display()
                ));
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                logging::event(format!(
                    "reload request cleanup failed {}: {error}",
                    path.display()
                ));
                false
            }
        }
    }
}

fn watch_event_affects_targets(event: &notify::Result<Event>, targets: &WatchTargets) -> bool {
    match event {
        Ok(event) => event_affects_targets(event, targets),
        Err(_) => true,
    }
}

fn event_affects_targets(event: &Event, targets: &WatchTargets) -> bool {
    event.paths.is_empty()
        || event.paths.iter().any(|path| {
            path_affects_config(path, &targets.sources, &targets.dotenv_path)
                || path_affects_plugins(path, targets.plugins_dir.as_deref())
                || targets
                    .reload_request_path
                    .as_deref()
                    .is_some_and(|request| paths_equivalent(path, request))
        })
}

fn path_affects_plugins(path: &Path, plugins_dir: Option<&Path>) -> bool {
    let Some(plugins_dir) = plugins_dir else {
        return false;
    };
    if paths_equivalent(path, plugins_dir) {
        return true;
    }
    path.extension().and_then(|ext| ext.to_str()) == Some("wasm")
        && path
            .parent()
            .is_some_and(|parent| paths_equivalent(parent, plugins_dir))
}

fn source_dirs(sources: &BTreeSet<PathBuf>, ignored_dir: Option<&Path>) -> BTreeSet<PathBuf> {
    sources
        .iter()
        .map(|source| source.parent().unwrap_or(Path::new(".")).to_path_buf())
        .filter(|dir| ignored_dir != Some(dir.as_path()))
        .collect()
}

fn watch_dir(watcher: &mut RecommendedWatcher, dir: &Path) -> Result<()> {
    watcher
        .watch(dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("watch config directory {}", dir.display()))
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    normalize_path(left) == normalize_path(right)
}

fn path_affects_config(path: &Path, sources: &BTreeSet<PathBuf>, dotenv_path: &Path) -> bool {
    sources.iter().any(|source| paths_equivalent(path, source))
        || sources
            .iter()
            .any(|source| paths_equivalent(path, source.parent().unwrap_or(Path::new("."))))
        || paths_equivalent(path, dotenv_path)
}

fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            break;
        };
        missing.push(name.to_os_string());
        let Some(parent) = ancestor.parent() else {
            break;
        };
        ancestor = parent;
    }

    let mut normalized = ancestor
        .canonicalize()
        .unwrap_or_else(|_| ancestor.to_path_buf());
    for component in missing.iter().rev() {
        normalized.push(component);
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_core::RawRule;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn config_fingerprint_is_stable_and_tracks_effective_rule_changes() {
        let mut document = ConfigDocument {
            rules: vec![RawRule {
                id: "one".into(),
                from: Some("cat".into()),
                to: Some("dog".into()),
                ..RawRule::default()
            }],
            ..ConfigDocument::default()
        };
        let original = config_fingerprint(&document).unwrap();

        assert_eq!(config_fingerprint(&document.clone()).unwrap(), original);
        document.rules[0].to = Some("fox".into());
        assert_ne!(config_fingerprint(&document).unwrap(), original);
    }

    #[test]
    fn source_fingerprint_tracks_contents_and_missing_dotenv() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.yaml");
        let imported = dir.path().join("rules.yaml");
        let dotenv = dir.path().join(".env");
        fs::write(&config, "rules: []\n").unwrap();
        fs::write(&imported, "rules: []\n").unwrap();
        let sources = BTreeSet::from([config.clone(), imported.clone()]);
        let original = source_contents_fingerprint(&sources, &dotenv).unwrap();

        assert_eq!(
            source_contents_fingerprint(&sources, &dotenv).unwrap(),
            original
        );
        fs::write(&imported, "rules:\n  - import: other.yaml\n").unwrap();
        assert_ne!(
            source_contents_fingerprint(&sources, &dotenv).unwrap(),
            original
        );
        fs::write(&imported, "rules: []\n").unwrap();
        fs::write(&dotenv, "TOKEN=changed\n").unwrap();
        assert_ne!(
            source_contents_fingerprint(&sources, &dotenv).unwrap(),
            original
        );
    }

    #[test]
    fn source_dirs_deduplicate_parent_directories() {
        let sources = [
            PathBuf::from("/tmp/config/config.yaml"),
            PathBuf::from("/tmp/config/rules/youtube.yaml"),
            PathBuf::from("/tmp/config/rules/urls.yaml"),
        ]
        .into_iter()
        .collect();

        let dirs = source_dirs(&sources, None);

        assert_eq!(dirs.len(), 2);
        assert!(dirs.contains(Path::new("/tmp/config")));
        assert!(dirs.contains(Path::new("/tmp/config/rules")));
    }

    #[test]
    fn dotenv_path_affects_config_even_before_the_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.yaml");
        let dotenv = dir.path().join(".env");
        let sources = [config].into_iter().collect();

        assert!(path_affects_config(&dotenv, &sources, &dotenv));
    }

    #[test]
    fn source_dirs_exclude_url_import_cache_dir() {
        let sources = [
            PathBuf::from("/tmp/config/config.yaml"),
            PathBuf::from("/tmp/state/url-imports/cached.yaml"),
        ]
        .into_iter()
        .collect();

        let dirs = source_dirs(&sources, Some(Path::new("/tmp/state/url-imports")));

        assert_eq!(dirs.len(), 1);
        assert!(dirs.contains(Path::new("/tmp/config")));
        assert!(!dirs.contains(Path::new("/tmp/state/url-imports")));
    }

    #[cfg(unix)]
    #[test]
    fn path_equivalence_resolves_symlinked_directories_in_both_directions() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        let linked = temp.path().join("linked");
        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &linked).unwrap();

        assert!(paths_equivalent(
            &real.join("config.yaml"),
            &linked.join("config.yaml")
        ));
        assert!(paths_equivalent(
            &linked.join("config.yaml"),
            &real.join("config.yaml")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn path_equivalence_handles_removed_atomic_save_target() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        let linked = temp.path().join("linked");
        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        let real_config = real.join("config.yaml");
        fs::write(&real_config, "rules: []").unwrap();
        let linked_config = linked.join("config.yaml");
        fs::remove_file(&real_config).unwrap();

        assert!(paths_equivalent(&real_config, &linked_config));
    }

    #[test]
    fn filesystem_events_wake_the_host_and_create_a_debounce_deadline() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.yaml");
        fs::write(
            &config_path,
            "config:\n  import_refresh_interval: 0\nrules: []\n",
        )
        .unwrap();
        let options = ConfigLoadOptions {
            state_dir: Some(temp.path().join("state")),
            refresh_url_imports: true,
            known_rule_types: BTreeSet::new(),
        };
        fs::create_dir_all(options.state_dir.as_ref().unwrap()).unwrap();
        let loaded = load_config_with_options(&config_path, options.clone()).unwrap();
        let wake_count = Arc::new(AtomicUsize::new(0));
        let callback_count = Arc::clone(&wake_count);
        let mut reloader = ConfigReloader::new(
            config_path.clone(),
            ConfigReloaderState {
                watched_sources: loaded.sources,
                remote_imports: loaded.remote_imports,
                import_refresh_interval: 0,
                document_fingerprint: config_fingerprint(&loaded.document).unwrap(),
                load_metadata_fingerprint: load_metadata_fingerprint(
                    &loaded.warnings,
                    &loaded.rule_sources,
                )
                .unwrap(),
            },
            options,
            ConfigReloaderHost {
                plugins_dir: None,
                wake: Some(Arc::new(move || {
                    callback_count.fetch_add(1, Ordering::Release);
                })),
            },
        )
        .unwrap();

        fs::write(
            &config_path,
            "config:\n  import_refresh_interval: 0\n  persist_last_clipboard: true\nrules: []\n",
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while wake_count.load(Ordering::Acquire) == 0 {
            assert!(Instant::now() < deadline, "watcher did not wake the host");
            std::thread::yield_now();
        }

        assert!(reloader.poll().unwrap().is_none());
        let debounce = reloader.next_deadline().expect("debounce deadline");
        assert!(debounce > Instant::now());
        assert!(debounce <= Instant::now() + RELOAD_DEBOUNCE);
    }

    #[test]
    fn state_outputs_do_not_enter_the_reload_event_queue() {
        let state_dir = PathBuf::from("/tmp/clipboard-transformer-state");
        let config = PathBuf::from("/tmp/clipboard-transformer-config/config.yaml");
        let dotenv = config.with_file_name(".env");
        let reload_request = state_dir.join(RELOAD_REQUEST_FILE);
        let targets = WatchTargets {
            sources: BTreeSet::from([config.clone()]),
            dotenv_path: dotenv.clone(),
            plugins_dir: Some(config.parent().unwrap().join("plugins")),
            reload_request_path: Some(reload_request.clone()),
        };

        for unrelated in [
            state_dir.join("clipboard-transformer.log"),
            state_dir.join("history.cbor"),
            state_dir.join("state.json"),
            state_dir.join("clipboard-transformer.pid"),
        ] {
            let event = Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Any))
                .add_path(unrelated);
            assert!(!watch_event_affects_targets(&Ok(event), &targets));
        }

        for relevant in [config, dotenv, reload_request] {
            let event = Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Any))
                .add_path(relevant);
            assert!(watch_event_affects_targets(&Ok(event), &targets));
        }
    }

    #[test]
    fn unchanged_url_refresh_skips_full_config_reload() {
        let body = b"rules:\n  - id: imported\n    from: cat\n    to: dog\n";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 1024];
                let _ = stream.read(&mut request).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"rules-v1\"\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(body).unwrap();
            }
        });
        let temp = tempfile::tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let config_path = temp.path().join("config.yaml");
        fs::write(
            &config_path,
            format!(
                "config:\n  import_refresh_interval: 1\nrules:\n  - import: http://{address}/rules.yaml\n"
            ),
        )
        .unwrap();
        let options = ConfigLoadOptions {
            state_dir: Some(state_dir),
            refresh_url_imports: true,
            known_rule_types: BTreeSet::new(),
        };
        let loaded = load_config_with_options(&config_path, options.clone()).unwrap();
        let document_fingerprint = config_fingerprint(&loaded.document).unwrap();
        let metadata_fingerprint =
            load_metadata_fingerprint(&loaded.warnings, &loaded.rule_sources).unwrap();
        let mut reloader = ConfigReloader::new(
            config_path,
            ConfigReloaderState {
                watched_sources: loaded.sources,
                remote_imports: loaded.remote_imports,
                import_refresh_interval: 1,
                document_fingerprint,
                load_metadata_fingerprint: metadata_fingerprint,
            },
            options,
            ConfigReloaderHost::default(),
        )
        .unwrap();
        // This test exercises the URL-refresh fast path. The watcher/debounce
        // contract is covered independently above, so detach its asynchronous
        // sources and discard any startup notifications before polling.
        for directory in reloader.watched_dirs.clone() {
            reloader.watcher.unwatch(&directory).unwrap();
        }
        if reloader.reload_request_watched {
            let directory = reloader
                .reload_request_path
                .as_deref()
                .and_then(Path::parent)
                .unwrap();
            reloader.watcher.unwatch(directory).unwrap();
        }
        while reloader.events.try_recv().is_ok() {}
        reloader.last_url_check = Instant::now() - Duration::from_secs(2);
        // If the fast path accidentally invokes the full loader, this missing
        // path makes the test fail instead of merely returning Unchanged.
        reloader.config_path = temp.path().join("missing.yaml");

        assert!(matches!(
            reloader.poll().unwrap(),
            Some(ReloadOutcome::Unchanged)
        ));
        server.join().unwrap();
    }
}
