use std::path::{Path, PathBuf};

use anyhow::Result;

use ct_core::RuleEngine;
use ct_notifications::StartupNotification;
use ct_runtime::app::runtime::RuntimeControl;
use ct_runtime::app::{Agent, AppCommand};
use ct_runtime::config::{
    ensure_default_config, load_config_with_options, ConfigLoadOptions, ConfigPaths,
};

/// Creates the tray and hands the main thread to `ct-host-loop`.
///
/// The tray gets one menu source, once, reading state the agent publishes when it
/// changes. So the menu — and the relative timestamps in it — are built when the
/// user opens it, and the loop never tells the tray to re-read anything.
fn run_host_loop<C, N>(
    mut runtime: ct_runtime::app::runtime::Runtime<'_, C, N>,
    tray_actions: ct_tray::ActionSink,
) -> Result<()>
where
    C: ct_clipboard::ClipboardBackend,
    N: ct_notifications::NotificationBackend,
{
    use ct_runtime::platform::tray::TrayStateHandle;

    let tray_state = TrayStateHandle::new(runtime.tray_snapshot());
    runtime.attach_tray_state(tray_state.clone());
    let mut tray = ct_tray::native::Tray::new(tray_actions, tray_state.source())?;
    ct_host_loop::run(|| {
        // The loop's vocabulary is this host's business, so the conversion lives
        // here rather than in `ct-runtime`, which owns no loop.
        match runtime.process_pending()? {
            RuntimeControl::Continue => {
                // The only per-iteration platform work left: the tray icon
                // follows the system light/dark preference, which no click
                // reports.
                tray.poll_chrome()?;
                Ok(ct_host_loop::Control::Continue)
            }
            RuntimeControl::Quit => Ok(ct_host_loop::Control::Quit),
        }
    })
}
use ct_runtime::{logging, platform, plugins};

/// Routes notification actions into the single command channel.
///
/// `ct-notifications` cannot name `AppCommand` — `objc2`'s `define_class!` is
/// not generic, so the macOS delegate stores a concrete callback in its ivars.
/// Converting here keeps every source on one channel, which is what preserves
/// ordering between a tray click and a notification action.
fn action_sink(commands: &std::sync::mpsc::Sender<AppCommand>) -> ct_notifications::ActionSink {
    let commands = commands.clone();
    Box::new(move |action| {
        if commands.send(AppCommand::from(action)).is_err() {
            logging::event("notification action dropped: command channel closed");
        }
    })
}

/// Same conversion for tray menu selections, onto the same channel.
fn tray_action_sink(commands: &std::sync::mpsc::Sender<AppCommand>) -> ct_tray::ActionSink {
    let commands = commands.clone();
    Box::new(move |action| {
        if commands.send(AppCommand::from(action)).is_err() {
            logging::event("tray action dropped: command channel closed");
        }
    })
}

/// Starts the native desktop host.
///
/// The app executable owns the watcher, tray, notifications, hot reload,
/// autostart integration, and single-instance behavior.
pub fn run() -> Result<()> {
    platform::bootstrap_host_environment();

    // One channel for every host event source. Tray clicks and notification
    // actions must stay ordered relative to each other, so they share this
    // sender instead of being funneled through per-source adapter threads.
    let (command_sender, command_receiver) = std::sync::mpsc::channel::<AppCommand>();
    let paths = ConfigPaths::resolve()?;
    logging::init(paths.state_dir.join("clipboard-transformer.log"));
    platform::verify_desktop_session()?;
    let config_path = resolve_or_create_config(&paths)?;
    paths.ensure_plugins_dir()?;
    let (dotenv, _) = platform::environment::load_dotenv_for_config(&config_path);
    logging::event(format!(
        "dotenv path={} present={} loaded={} ignored={}",
        dotenv.path.display(),
        dotenv.present,
        dotenv.loaded_count,
        dotenv.ignored_count
    ));
    logging::event(format!("desktop config={}", config_path.display()));

    let catalog = plugins::PluginCatalog::discover(&paths.plugins_dir);
    let schema_contributions = ct_runtime::config::plugin_schema_contributions(&catalog);
    let load_options = ConfigLoadOptions {
        state_dir: Some(paths.state_dir.clone()),
        refresh_url_imports: true,
        known_rule_types: catalog.known_rule_types(),
    };
    let mut loaded = match load_config_with_options(&config_path, load_options.clone()) {
        Ok(loaded) => loaded,
        Err(error) => {
            deliver_startup_failure(
                &config_path,
                &error,
                ct_runtime::config::AppConfig::default().disable_for,
            );
            return Err(error);
        }
    };
    let disable_for = loaded.document.config.disable_for;
    let document_fingerprint = ct_runtime::app::reload::config_fingerprint(&loaded.document)?;
    let load_metadata_fingerprint =
        ct_runtime::app::reload::load_metadata_fingerprint(&loaded.warnings, &loaded.rule_sources)?;
    let plugin_set = catalog.initialize_for_rules(
        &loaded.document.plugins,
        &plugins::PluginLimits::default(),
        &loaded.document.rules,
    );
    log_plugin_summary(&plugin_set);
    let rules = std::mem::take(&mut loaded.document.rules);
    let shell_paths = ct_runtime::rules::shell::ShellHostPaths::from_config_paths(
        &paths,
        Some(config_path.clone()),
    );
    let external_providers = ct_runtime::rules::external_providers(
        &loaded.document.config,
        shell_paths,
        &loaded.rule_sources,
        plugin_set.providers(),
    );
    let (engine, skipped_rules) =
        match RuleEngine::compile_with_external(rules, &external_providers) {
            Ok(compiled) => compiled,
            Err(error) => {
                deliver_startup_failure(&config_path, &error, disable_for);
                return Err(error);
            }
        };
    let plugin_fingerprint = plugin_set.issue_fingerprint();
    let plugin_statuses = plugin_set.into_statuses();
    sync_runtime_schema(&config_path, &schema_contributions)?;
    let rule_count = engine.rule_count();
    let mut config_warnings = std::mem::take(&mut loaded.warnings);
    config_warnings.extend(skipped_rules.into_iter().map(skipped_rule_warning));
    let rule_sources = std::mem::take(&mut loaded.rule_sources);
    let source_count = loaded.sources.len();
    let watched_sources = std::mem::take(&mut loaded.sources);
    let import_refresh_interval = loaded.document.config.import_refresh_interval;
    let agent_document = loaded.document;
    let mut reloader = ct_runtime::app::reload::ConfigReloader::new(
        config_path.clone(),
        watched_sources,
        import_refresh_interval,
        document_fingerprint,
        load_metadata_fingerprint,
        load_options,
        Some(paths.plugins_dir.clone()),
    )?;

    let _instance_guard =
        platform::instance_guard(&paths.state_dir.join("clipboard-transformer.pid"))?;

    // One path for every platform: each backend picks its own implementation,
    // and `platform::host` holds the remaining per-OS decisions.
    let clipboard = match ct_clipboard::native::backend() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            platform::present_runtime_failure(&error);
            return Err(error);
        }
    };
    let notifications = match platform::notification_backend(
        action_sink(&command_sender),
        ct_runtime::APP_USER_MODEL_ID,
        disable_for,
    ) {
        Ok(notifications) => notifications,
        Err(error) => {
            platform::present_runtime_failure(&error);
            return Err(error);
        }
    };
    let mut agent = match Agent::new_with_engine(clipboard, notifications, agent_document, engine) {
        Ok(agent) => agent,
        Err(error) => {
            deliver_startup_failure(&config_path, &error, disable_for);
            return Err(error);
        }
    };
    prepare_agent(
        &mut agent,
        &config_path,
        &paths,
        rule_sources,
        source_count,
        config_warnings,
        (plugin_statuses, plugin_fingerprint),
    )?;
    agent.deliver_startup_notification(&config_path, rule_count)?;
    agent.deliver_plugin_attention_notification();

    if let Err(error) = run_host_loop(
        ct_runtime::app::runtime::Runtime::new(&mut agent, Some(&mut reloader), &command_receiver),
        tray_action_sink(&command_sender),
    ) {
        platform::present_runtime_failure(&error);
        return Err(error);
    }

    Ok(())
}

fn prepare_agent<C, N>(
    agent: &mut Agent<C, N>,
    config_path: &Path,
    paths: &ConfigPaths,
    rule_sources: std::collections::BTreeMap<String, ct_runtime::config::RuleSource>,
    source_count: usize,
    config_warnings: Vec<ct_runtime::config::ConfigWarning>,
    plugin_state: (Vec<plugins::PluginStatus>, u64),
) -> Result<()>
where
    C: ct_clipboard::ClipboardBackend,
    N: ct_notifications::NotificationBackend,
{
    agent.set_edit_config_path(config_path.to_path_buf());
    agent.set_rule_sources(rule_sources);
    agent.set_tray_source_count(source_count);
    agent.set_tray_config_warnings(config_warnings);
    agent.set_plugin_statuses(plugin_state.0, plugin_state.1);
    agent.set_autostart_status(platform::autostart::status());
    agent.load_persistent_state(paths.state_dir.clone())
}

fn resolve_or_create_config(paths: &ConfigPaths) -> Result<PathBuf> {
    if paths.config_file.exists() {
        Ok(paths.config_file.clone())
    } else {
        let toml = paths.config_dir.join("config.toml");
        if toml.exists() {
            Ok(toml)
        } else {
            let created = ensure_default_config(&paths.config_file)?;
            if created {
                logging::event(format!(
                    "created default config at {}",
                    paths.config_file.display()
                ));
            }
            Ok(paths.config_file.clone())
        }
    }
}

fn sync_runtime_schema(
    config_path: &Path,
    plugins: &[ct_runtime::config::PluginRuleSchemaContribution],
) -> Result<()> {
    let schema = ct_runtime::config::json_schema_pretty_with_plugins(plugins)?;
    if let Some(schema_path) =
        ct_runtime::config::sync_config_schema_contents_next_to(config_path, &schema)?
    {
        logging::event(format!(
            "wrote runtime config schema at {}",
            schema_path.display()
        ));
    }
    Ok(())
}

fn skipped_rule_warning(
    skipped: ct_core::SkippedExternalRule,
) -> ct_runtime::config::ConfigWarning {
    ct_runtime::config::ConfigWarning::InvalidRule {
        id: Some(skipped.id),
        kind: skipped.kind,
        reason: skipped.reason,
    }
}

fn log_plugin_summary(set: &plugins::PluginSet) {
    for status in set.statuses() {
        logging::event(format!(
            "plugin {} state={} rules={} path={}",
            status.id,
            status.state.as_str(),
            status.available_rules.len(),
            status.path.display()
        ));
        for issue in &status.issues {
            logging::event(format!(
                "plugin {} issue {} ({:?}): {}",
                status.id, issue.code, issue.severity, issue.summary
            ));
        }
    }
}

fn deliver_startup_failure(config_path: &Path, error: &anyhow::Error, disable_for: u64) {
    platform::deliver_startup_failure(
        StartupNotification {
            notification_id: "clipboard-transformer-startup-failed".to_string(),
            title: "Clipboard Transformer failed to start".to_string(),
            body: error.to_string(),
            edit_target: Some(ct_notifications::EditTarget {
                path: config_path.display().to_string(),
                line: None,
            }),
            reload_request_path: None,
        },
        disable_for,
    );
}
