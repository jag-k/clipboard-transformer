use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use ct_clipboard::{ClipboardBackend, ClipboardItem};
use ct_core::RuleEngine;
use ct_runtime::config::{
    ensure_default_config, load_config_with_options, validate_loaded_config,
    write_config_schema_next_to, ConfigFormat, ConfigLoadOptions, ConfigPaths,
};

#[cfg(target_os = "macos")]
const APP_BUNDLE_NAME: &str = "Clipboard Transformer.app";
#[cfg(target_os = "macos")]
const APP_EXECUTABLE_NAME: &str = "Clipboard Transformer";

#[derive(Debug, Parser)]
#[command(name = "clipboard-transformer")]
#[command(version)]
#[command(
    about = "Transform clipboard content with the same rules as the desktop app",
    long_about = "Transform clipboard content with the same rules as the desktop app.\n\n\
                  The CLI is an optional tool for terminal workflows, scripts, and \
                  diagnostics. It never starts the desktop app or a background daemon.",
    after_help = "Examples:\n  \
                  clipboard-transformer config check\n  \
                  clipboard-transformer transform\n  \
                  clipboard-transformer transform --preview\n  \
                  printf '%s' 'https://example.com/?utm_source=test' | clipboard-transformer transform -\n  \
                  clipboard-transformer rules list\n  \
                  clipboard-transformer clipboard watch --format jsonl"
)]
#[command(subcommand_required = true, arg_required_else_help = true)]
pub struct Cli {
    #[command(subcommand)]
    command: CommandKind,
}

#[derive(Debug, Subcommand)]
enum CommandKind {
    #[cfg(unix)]
    #[command(name = "__dump-environment", hide = true)]
    DumpEnvironment { marker: String },
    /// Transform the current clipboard, or use `-` as a stdin/stdout filter.
    #[command(after_help = "Examples:\n  \
                      clipboard-transformer transform\n  \
                      clipboard-transformer transform --preview\n  \
                      printf '%s' 'https://example.com/?utm_source=test' | \\\n    \
                      clipboard-transformer transform - --config-file ./config.yaml")]
    Transform {
        /// Input source. Omit it to use the current clipboard; `-` means stdin/stdout.
        #[arg(value_name = "SOURCE")]
        source: Option<String>,
        /// Show how the current clipboard would change without writing it.
        #[arg(long, visible_alias = "dry-run")]
        preview: bool,
        /// Treat stdin as this clipboard format instead of plain text.
        #[arg(long, value_name = "FORMAT")]
        input_format: Option<String>,
        #[command(flatten)]
        inputs: ConfigInputs,
    },
    /// Discover rule types and inspect configured rule representations.
    #[command(after_help = "Examples:\n  \
                      clipboard-transformer rules list\n  \
                      clipboard-transformer rules list --available-only --format json\n  \
                      clipboard-transformer rules view effective\n  \
                      clipboard-transformer rules view effective --format yaml")]
    Rules {
        #[command(subcommand)]
        command: RulesCommand,
    },
    /// Read or monitor the native clipboard without modifying it.
    #[command(after_help = "Examples:\n  \
                      clipboard-transformer clipboard inspect\n  \
                      clipboard-transformer clipboard watch --format jsonl\n  \
                      clipboard-transformer clipboard watch --transform=transformed-only")]
    Clipboard {
        #[command(subcommand)]
        command: ClipboardCommand,
    },
    /// Create, check, and inspect configuration.
    #[command(after_help = "Examples:\n  \
                      clipboard-transformer config check\n  \
                      clipboard-transformer config init --config-file ./config.yaml")]
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Inspect and diagnose WASM rule plugins.
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },
    /// Print paths for config, plugins, state, cache, and any discovered desktop app.
    Paths,
    /// Diagnose platform capabilities and the resolved installation.
    Doctor,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Create a starter YAML config and adjacent JSON Schema.
    ///
    /// Existing configuration is never overwritten.
    Init {
        /// YAML file to create. Defaults to the system config path.
        #[arg(long, value_name = "PATH")]
        config_file: Option<PathBuf>,
    },
    /// Load config, expand imports, compile rules, and report problems.
    Check {
        #[command(flatten)]
        inputs: ConfigInputs,
    },
    /// Print or write the config JSON Schema.
    Schema {
        /// Write the schema to a file instead of stdout.
        #[arg(short, long, value_name = "PATH")]
        output: Option<PathBuf>,
        /// Generate the built-in-only schema without discovering plugins.
        #[arg(long, conflicts_with = "plugins")]
        no_plugins: bool,
        /// Include only these plugin ids (repeatable). Default: every discovered plugin.
        #[arg(long = "plugin", value_name = "ID")]
        plugins: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
enum ClipboardCommand {
    /// Describe the latest stored clipboard item, falling back to a live read.
    Inspect,
    /// Print new clipboard items without writing changes back.
    ///
    /// Config, plugin, and state options require `--transform`. Without it,
    /// this command only observes the native clipboard.
    Watch {
        /// Event output format.
        #[arg(long, value_enum, default_value_t = WatchFormat::Text)]
        format: WatchFormat,
        /// Apply configured rules before printing.
        ///
        /// With no value, print original and final items. `transformed-only`
        /// skips items that match no rule.
        #[arg(
            long,
            value_enum,
            num_args = 0..=1,
            default_missing_value = "both"
        )]
        transform: Option<WatchTransformMode>,
        #[command(flatten)]
        inputs: ConfigInputs,
    },
}

#[derive(Debug, Default, Args)]
struct ConfigInputs {
    /// Self-contained inline YAML or TOML configuration.
    #[arg(long, conflicts_with = "config_file", value_name = "DOCUMENT")]
    config: Option<String>,
    /// YAML or TOML file. Defaults to the active system config.
    #[arg(long, value_name = "PATH")]
    config_file: Option<PathBuf>,
    /// Discover plugins in this directory.
    ///
    /// Inline config loads no plugins unless this option is set.
    #[arg(long, value_name = "PATH")]
    plugin_dir: Option<PathBuf>,
    /// State directory used for the URL import cache.
    #[arg(long, value_name = "PATH", conflicts_with = "config")]
    state_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum WatchFormat {
    Jsonl,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum WatchTransformMode {
    Both,
    TransformedOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum RulesView {
    Effective,
}

#[derive(Debug, Subcommand)]
enum RulesCommand {
    /// List built-in, native shell, and discovered plugin rule types.
    List {
        #[command(flatten)]
        inputs: ConfigInputs,
        /// Output format.
        #[arg(long, value_enum, default_value_t = RuleListFormat::Text)]
        format: RuleListFormat,
        /// Omit disabled shell rules and unavailable plugin rule types.
        #[arg(long)]
        available_only: bool,
    },
    /// Print one representation of the configured rule instances.
    View {
        /// Representation to print.
        #[arg(value_enum, value_name = "VIEW")]
        view: RulesView,
        #[command(flatten)]
        inputs: ConfigInputs,
        /// Serialization format.
        #[arg(long, value_enum, default_value_t = RuleViewFormat::Json)]
        format: RuleViewFormat,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RuleListFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RuleViewFormat {
    Json,
    Yaml,
}

#[derive(Debug, Serialize)]
struct RuleCatalog {
    version: u32,
    config_path: Option<PathBuf>,
    rules: Vec<RuleCatalogEntry>,
}

#[derive(Debug, Serialize)]
struct RuleCatalogEntry {
    rule_type: String,
    source: String,
    available: bool,
    status: String,
    scope: &'static str,
    default_formats: Vec<String>,
    description: String,
}

struct TransformPipeline {
    engine: RuleEngine,
    app_matcher: ct_core::AppMatcher,
    max_item_bytes: u64,
}

struct LoadedCliConfig {
    loaded: ct_runtime::config::LoadedConfig,
    config_path: Option<PathBuf>,
    paths: ConfigPaths,
    catalog: Option<ct_runtime::plugins::PluginCatalog>,
}

#[derive(Debug, Subcommand)]
enum PluginCommand {
    /// List discovered plugins and their states.
    List,
    /// Show one plugin's manifest, capabilities, rules, and issues.
    Inspect {
        /// Plugin manifest id.
        #[arg(value_name = "ID")]
        id: String,
    },
    /// Report every plugin issue in detail.
    Doctor {
        /// Diagnose only this plugin.
        #[arg(value_name = "ID")]
        id: Option<String>,
    },
    /// Print a copyable configuration example for one plugin.
    Example {
        /// Plugin manifest id.
        #[arg(value_name = "ID")]
        id: String,
    },
    /// Print the directories scanned for plugin modules.
    Paths,
    /// Download a plugin module over HTTPS and place it into the plugins
    /// directory after validating its embedded manifest.
    Install {
        /// HTTPS URL of the WASM plugin module.
        #[arg(value_name = "URL")]
        url: String,
    },
    /// Ask a running desktop instance to reload config and plugins.
    Reload,
}

pub fn run() -> Result<()> {
    run_cli(Cli::parse())
}

fn run_cli(cli: Cli) -> Result<()> {
    match cli.command {
        #[cfg(unix)]
        CommandKind::DumpEnvironment { marker } => {
            ct_runtime::platform::environment::dump_current_environment(&marker)?;
            Ok(())
        }
        CommandKind::Paths => print_paths(),
        CommandKind::Doctor => doctor(),
        CommandKind::Config { command } => match command {
            ConfigCommand::Init { config_file } => init_config(config_file),
            ConfigCommand::Check { inputs } => validate(inputs),
            ConfigCommand::Schema {
                output,
                no_plugins,
                plugins,
            } => print_schema(output, no_plugins, plugins),
        },
        CommandKind::Clipboard { command } => match command {
            ClipboardCommand::Inspect => inspect_clipboard(),
            ClipboardCommand::Watch {
                format,
                transform,
                inputs,
            } => watch_clipboard(format, transform, inputs),
        },
        CommandKind::Rules { command } => run_rules_command(command),
        CommandKind::Transform {
            source,
            preview,
            input_format,
            inputs,
        } => transform(source.as_deref(), preview, input_format.as_deref(), inputs),
        CommandKind::Plugin { command } => run_plugin_command(command),
    }
}

fn native_clipboard_backend() -> Result<Box<dyn ClipboardBackend>> {
    let backend = ct_clipboard::native::backend();
    #[cfg(target_os = "linux")]
    {
        backend.with_context(|| {
            format!(
                "Linux clipboard setup: {}",
                ct_runtime::platform::linux::diagnostics::SUPPORT_URL
            )
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        backend
    }
}

fn transform(
    source: Option<&str>,
    preview: bool,
    input_format: Option<&str>,
    inputs: ConfigInputs,
) -> Result<()> {
    match source {
        Some("-") if preview => {
            bail!("--preview is only valid for the current clipboard; omit `-`")
        }
        Some("-") => transform_stdin(inputs, input_format),
        Some(source) => bail!("unsupported transform source {source:?}; use `-` or omit it"),
        None if input_format.is_some() => {
            bail!("--input-format is only valid for stdin; pass `-`")
        }
        None => transform_clipboard(inputs, preview),
    }
}

fn transform_stdin(inputs: ConfigInputs, input_format: Option<&str>) -> Result<()> {
    let mut stdin = String::new();
    std::io::stdin()
        .read_to_string(&mut stdin)
        .context("read UTF-8 input from stdin")?;
    let mut pipeline = load_transform_pipeline(&inputs)?;
    let input = if let Some(format) = input_format {
        let mut input = ClipboardItem::from_text("");
        input.set(ct_clipboard::normalize_format(format)?, stdin.clone());
        input
    } else {
        ClipboardItem::from_text(&stdin)
    };
    let output = pipeline
        .engine
        .try_apply(&input)?
        .and_then(|result| result.after.text().map(str::to_owned))
        .unwrap_or(stdin);
    std::io::stdout()
        .write_all(output.as_bytes())
        .context("write transformed text to stdout")
}

fn transform_clipboard(inputs: ConfigInputs, preview: bool) -> Result<()> {
    let mut pipeline = load_transform_pipeline(&inputs)?;
    let mut clipboard = native_clipboard_backend()?;
    let metadata = clipboard.metadata()?;
    if metadata.is_ignored() {
        bail!("current clipboard item is marked as non-transformable");
    }
    if !pipeline.app_matcher.allows_app(metadata.source_app()) {
        println!("no match");
        return Ok(());
    }

    let required_formats = pipeline.engine.required_formats().clone();
    let Some(input) = clipboard.read_formats_limited(&required_formats, pipeline.max_item_bytes)?
    else {
        println!("no match");
        return Ok(());
    };
    let input = input.with_optional_source_app(metadata.source_app().cloned());
    let Some(result) = pipeline.engine.try_apply(&input)? else {
        println!("no match");
        return Ok(());
    };

    if !preview {
        clipboard.write(&result.after)?;
    }
    println!(
        "{} rules={}",
        if preview {
            "would transform"
        } else {
            "transformed"
        },
        result.applied_rule_ids().collect::<Vec<_>>().join(",")
    );
    for (format, value) in result.after.text_representations() {
        println!("{}={}", format.as_str(), value);
    }
    if let Some(message) = result.message {
        println!("message={message}");
    }
    Ok(())
}

fn load_transform_pipeline(inputs: &ConfigInputs) -> Result<TransformPipeline> {
    let LoadedCliConfig {
        loaded,
        config_path,
        paths,
        catalog,
    } = load_cli_config(inputs)?;
    let rule_sources = loaded.rule_sources;
    let document = loaded.document;

    let app_matcher = document.config.app_matcher()?;
    let max_item_bytes = document.config.max_item_bytes;
    let shell_paths =
        ct_runtime::rules::shell::ShellHostPaths::from_config_paths(&paths, config_path);
    let (engine, skipped) = if let Some(catalog) = catalog {
        let plugin_set = catalog.initialize_for_rules(
            &document.plugins,
            &ct_runtime::plugins::PluginLimits::default(),
            &document.rules,
        );
        let providers = ct_runtime::rules::external_providers(
            &document.config,
            shell_paths,
            &rule_sources,
            plugin_set.providers(),
        );
        RuleEngine::compile_with_external(document.rules, &providers)?
    } else {
        let providers = ct_runtime::rules::external_providers(
            &document.config,
            shell_paths,
            &rule_sources,
            &[],
        );
        RuleEngine::compile_with_external(document.rules, &providers)?
    };
    for skipped in skipped {
        eprintln!(
            "warning: rule {:?} ({}) skipped: {}",
            skipped.id, skipped.kind, skipped.reason
        );
    }
    Ok(TransformPipeline {
        engine,
        app_matcher,
        max_item_bytes,
    })
}

fn load_cli_config(inputs: &ConfigInputs) -> Result<LoadedCliConfig> {
    let mut paths = ConfigPaths::resolve()?;
    if let Some(state_dir) = &inputs.state_dir {
        paths.state_dir = state_dir.clone();
    }
    let plugin_dir = inputs
        .plugin_dir
        .clone()
        .or_else(|| inputs.config.is_none().then(|| paths.plugins_dir.clone()));
    let catalog = plugin_dir
        .as_deref()
        .map(ct_runtime::plugins::PluginCatalog::discover);
    let known_rule_types = catalog
        .as_ref()
        .map(ct_runtime::plugins::PluginCatalog::known_rule_types)
        .unwrap_or_default();

    let (loaded, config_path) = if let Some(config) = &inputs.config {
        (
            ct_runtime::config::load_inline_config(config, known_rule_types)?,
            None,
        )
    } else {
        let config_path = resolve_config(inputs.config_file.clone())?;
        ct_runtime::platform::environment::load_dotenv_for_config(&config_path);
        let loaded = load_config_with_options(
            &config_path,
            ConfigLoadOptions {
                state_dir: Some(paths.state_dir.clone()),
                refresh_url_imports: true,
                known_rule_types,
            },
        )?;
        (loaded, Some(config_path))
    };
    Ok(LoadedCliConfig {
        loaded,
        config_path,
        paths,
        catalog,
    })
}

fn load_rule_catalog_config(inputs: &ConfigInputs) -> Result<LoadedCliConfig> {
    if inputs.config.is_none() && inputs.config_file.is_none() {
        let mut paths = ConfigPaths::resolve()?;
        if let Some(state_dir) = &inputs.state_dir {
            paths.state_dir = state_dir.clone();
        }
        let toml_config = paths.config_dir.join("config.toml");
        if !paths.config_file.exists() && !toml_config.exists() {
            let plugin_dir = inputs
                .plugin_dir
                .clone()
                .unwrap_or_else(|| paths.plugins_dir.clone());
            return Ok(LoadedCliConfig {
                loaded: ct_runtime::config::LoadedConfig {
                    document: ct_runtime::config::ConfigDocument::default(),
                    sources: Default::default(),
                    rule_sources: Default::default(),
                    warnings: Vec::new(),
                },
                config_path: None,
                paths,
                catalog: Some(ct_runtime::plugins::PluginCatalog::discover(&plugin_dir)),
            });
        }
    }
    load_cli_config(inputs)
}

fn watch_clipboard(
    format: WatchFormat,
    transform: Option<WatchTransformMode>,
    inputs: ConfigInputs,
) -> Result<()> {
    if transform.is_none()
        && (inputs.config.is_some()
            || inputs.config_file.is_some()
            || inputs.plugin_dir.is_some()
            || inputs.state_dir.is_some())
    {
        bail!("watch config/plugin/state options require --transform");
    }
    let mut pipeline = transform
        .map(|_| load_transform_pipeline(&inputs))
        .transpose()?;

    // The platform choice lives inside `ct_clipboard::native`, not here.
    let mut clipboard = native_clipboard_backend()?;

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        let mut last_change_count = clipboard.change_count()?;
        loop {
            std::thread::sleep(Duration::from_millis(100));
            let change_count = clipboard.change_count()?;
            if change_count.is_some() && change_count == last_change_count {
                continue;
            }
            last_change_count = change_count;
            let metadata = clipboard.metadata()?;
            if metadata.is_ignored() {
                continue;
            }
            if pipeline
                .as_ref()
                .is_some_and(|pipeline| !pipeline.app_matcher.allows_app(metadata.source_app()))
            {
                continue;
            }
            let max_item_bytes = pipeline
                .as_ref()
                .map_or(0, |pipeline| pipeline.max_item_bytes);
            let Some(item) = clipboard.read_limited(max_item_bytes)? else {
                continue;
            };
            if change_count.is_some() && clipboard.change_count()? != change_count {
                continue;
            }
            let item = item.with_optional_source_app(metadata.source_app().cloned());
            let transformed = pipeline
                .as_mut()
                .map(|pipeline| pipeline.engine.try_apply(&item))
                .transpose()?
                .flatten();
            if transform == Some(WatchTransformMode::TransformedOnly) && transformed.is_none() {
                continue;
            }
            print_watch_item(format, transform, change_count, &item, transformed.as_ref())?;
        }
    }
}

fn print_watch_item(
    format: WatchFormat,
    transform: Option<WatchTransformMode>,
    change_count: Option<u64>,
    original: &ClipboardItem,
    transformed: Option<&ct_core::TransformResult>,
) -> Result<()> {
    match format {
        WatchFormat::Jsonl => {
            let mut event = serde_json::json!({
                "observed_at": chrono::Utc::now().to_rfc3339(),
                "change_count": change_count,
            });
            let object = event.as_object_mut().expect("JSON object");
            match transform {
                None => {
                    object.insert("item".into(), serde_json::to_value(original)?);
                }
                Some(WatchTransformMode::Both) => {
                    object.insert("original".into(), serde_json::to_value(original)?);
                    object.insert(
                        "transformed".into(),
                        transformed
                            .map(|result| serde_json::to_value(&result.after))
                            .transpose()?
                            .unwrap_or(serde_json::Value::Null),
                    );
                }
                Some(WatchTransformMode::TransformedOnly) => {
                    let result = transformed.expect("non-matches are skipped");
                    object.insert("item".into(), serde_json::to_value(&result.after)?);
                }
            }
            if let Some(result) = transformed {
                object.insert(
                    "applied_rules".into(),
                    serde_json::json!(result.applied_rule_ids().collect::<Vec<_>>()),
                );
            }
            println!("{}", serde_json::to_string(&event)?);
        }
        WatchFormat::Text => match transform {
            None => println!("{}", watch_text(original)),
            Some(WatchTransformMode::Both) => {
                println!(
                    "original={} transformed={}",
                    serde_json::to_string(&watch_text(original))?,
                    transformed
                        .map(|result| serde_json::to_string(&watch_text(&result.after)))
                        .transpose()?
                        .unwrap_or_else(|| "null".to_string())
                );
            }
            Some(WatchTransformMode::TransformedOnly) => println!(
                "{}",
                watch_text(&transformed.expect("non-matches are skipped").after)
            ),
        },
    }
    std::io::stdout().flush().context("flush clipboard event")
}

fn watch_text(item: &ClipboardItem) -> String {
    item.text().map(str::to_string).unwrap_or_else(|| {
        format!(
            "[clipboard item: {} formats, {} bytes]",
            item.representations().len(),
            item.size_bytes()
        )
    })
}

fn print_paths() -> Result<()> {
    let paths = ConfigPaths::resolve()?;
    println!("config_dir={}", paths.config_dir.display());
    println!("config_file={}", paths.config_file.display());
    println!("plugins_dir={}", paths.plugins_dir.display());
    println!("state_dir={}", paths.state_dir.display());
    println!("cache_dir={}", paths.cache_dir.display());
    #[cfg(target_os = "macos")]
    if let Some(bundle) = default_app_bundle_path() {
        println!("app_bundle={}", bundle.display());
        println!("app_executable={}", app_executable_path(&bundle).display());
    }
    Ok(())
}

fn validate(inputs: ConfigInputs) -> Result<()> {
    let loaded = load_cli_config(&inputs)?;
    let known_rule_types = loaded
        .catalog
        .as_ref()
        .map(ct_runtime::plugins::PluginCatalog::known_rule_types)
        .unwrap_or_default();
    let report = validate_loaded_config(loaded.loaded, &known_rule_types)?;
    if report.is_clean() {
        println!("config ok");
    } else {
        for warning in report.warnings {
            println!("warning: {warning}");
        }
    }
    Ok(())
}

fn run_rules_command(command: RulesCommand) -> Result<()> {
    match command {
        RulesCommand::List {
            inputs,
            format,
            available_only,
        } => print_rule_catalog(inputs, format, available_only),
        RulesCommand::View {
            view,
            inputs,
            format,
        } => print_rule_view(inputs, view, format),
    }
}

fn print_rule_view(inputs: ConfigInputs, view: RulesView, format: RuleViewFormat) -> Result<()> {
    let loaded = load_cli_config(&inputs)?;
    let output = match view {
        RulesView::Effective => effective_rules_view(&loaded.loaded, loaded.config_path.as_deref()),
    };
    match format {
        RuleViewFormat::Json => println!("{}", serde_json::to_string_pretty(&output)?),
        RuleViewFormat::Yaml => print!("{}", serde_yaml::to_string(&output)?),
    }
    Ok(())
}

fn print_rule_catalog(
    inputs: ConfigInputs,
    format: RuleListFormat,
    available_only: bool,
) -> Result<()> {
    let catalog = build_rule_catalog(inputs, available_only)?;
    match format {
        RuleListFormat::Json => println!("{}", serde_json::to_string_pretty(&catalog)?),
        RuleListFormat::Text => print_rule_catalog_text(&catalog),
    }
    Ok(())
}

fn build_rule_catalog(inputs: ConfigInputs, available_only: bool) -> Result<RuleCatalog> {
    let LoadedCliConfig {
        loaded,
        config_path,
        catalog,
        ..
    } = load_rule_catalog_config(&inputs)?;
    let mut rules = builtin_rule_catalog();

    let shell_enabled = loaded.document.config.shell.enabled;
    for (rule_type, scope, description) in [
        (
            "shell",
            "text",
            "Run a trusted native command as a text transform.",
        ),
        (
            "item-shell",
            "item",
            "Run a trusted native command against a complete clipboard item.",
        ),
    ] {
        rules.push(RuleCatalogEntry {
            rule_type: rule_type.to_string(),
            source: "native".to_string(),
            available: shell_enabled,
            status: if shell_enabled {
                "available".to_string()
            } else {
                "disabled".to_string()
            },
            scope,
            default_formats: vec!["text".to_string()],
            description: description.to_string(),
        });
    }

    if let Some(catalog) = catalog {
        let plugin_set = catalog.initialize(
            &loaded.document.plugins,
            &ct_runtime::plugins::PluginLimits::default(),
        );
        for status in plugin_set.statuses() {
            let Some(manifest) = &status.manifest else {
                eprintln!(
                    "warning: plugin module {} has no readable manifest",
                    status.path.display()
                );
                continue;
            };
            for descriptor in &manifest.rules {
                let rule_type =
                    ct_plugin_api::namespaced_rule_type(&manifest.id, &descriptor.rule_type);
                let available = status.available_rules.contains(&rule_type);
                rules.push(RuleCatalogEntry {
                    rule_type,
                    source: format!("plugin:{}", manifest.id),
                    available,
                    status: status.state.as_str().to_string(),
                    scope: "text",
                    default_formats: if descriptor.formats.is_empty() {
                        vec!["text".to_string()]
                    } else {
                        descriptor.formats.clone()
                    },
                    description: descriptor
                        .description
                        .clone()
                        .or_else(|| descriptor.name.clone())
                        .unwrap_or_else(|| format!("Rule provided by {}.", manifest.name)),
                });
            }
        }
    }

    if available_only {
        rules.retain(|rule| rule.available);
    }
    Ok(RuleCatalog {
        version: 1,
        config_path,
        rules,
    })
}

fn builtin_rule_catalog() -> Vec<RuleCatalogEntry> {
    [
        (
            "regexp",
            "text",
            &["text"][..],
            "Replace text with a Rust regular expression.",
        ),
        (
            "url",
            "text",
            &["text"][..],
            "Apply one encoding-safe structural URL transform.",
        ),
        (
            "url-cleanup",
            "text",
            &["text"][..],
            "Remove selected query parameters from HTTP(S) URLs.",
        ),
        (
            "ruleset",
            "composite",
            &[][..],
            "Compose nested rules with an explicit match policy.",
        ),
    ]
    .into_iter()
    .map(
        |(rule_type, scope, default_formats, description)| RuleCatalogEntry {
            rule_type: rule_type.to_string(),
            source: "built-in".to_string(),
            available: true,
            status: "available".to_string(),
            scope,
            default_formats: default_formats.iter().map(ToString::to_string).collect(),
            description: description.to_string(),
        },
    )
    .collect()
}

fn print_rule_catalog_text(catalog: &RuleCatalog) {
    let type_width = catalog
        .rules
        .iter()
        .map(|rule| rule.rule_type.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let source_width = catalog
        .rules
        .iter()
        .map(|rule| rule.source.len())
        .max()
        .unwrap_or(6)
        .max(6);
    let status_width = catalog
        .rules
        .iter()
        .map(|rule| rule.status.len())
        .max()
        .unwrap_or(6)
        .max(6);
    println!(
        "{:<type_width$}  {:<source_width$}  {:<status_width$}  {:<9}  {:<12}  DESCRIPTION",
        "TYPE", "SOURCE", "STATUS", "SCOPE", "FORMATS"
    );
    for rule in &catalog.rules {
        let formats = if rule.default_formats.is_empty() {
            "children".to_string()
        } else {
            rule.default_formats.join(",")
        };
        println!(
            "{:<type_width$}  {:<source_width$}  {:<status_width$}  {:<9}  {:<12}  {}",
            rule.rule_type, rule.source, rule.status, rule.scope, formats, rule.description
        );
    }
}

fn effective_rules_view(
    loaded: &ct_runtime::config::LoadedConfig,
    config_path: Option<&Path>,
) -> serde_json::Value {
    let rules = loaded
        .document
        .rules
        .iter()
        .map(effective_rule_value)
        .collect::<Vec<_>>();
    let warnings = loaded
        .warnings
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    serde_json::json!({
        "version": 1,
        "view": "effective",
        "config_path": config_path,
        "sources": &loaded.sources,
        "rule_sources": &loaded.rule_sources,
        "warnings": warnings,
        "rules": rules,
    })
}

fn effective_rule_value(rule: &ct_core::RawRule) -> serde_json::Value {
    let kind = rule.kind.as_deref().unwrap_or("regexp");
    let mut object = serde_json::Map::new();
    object.insert("type".into(), serde_json::Value::String(kind.to_string()));
    object.insert("id".into(), serde_json::Value::String(rule.id.clone()));
    if let Some(name) = &rule.name {
        object.insert("name".into(), serde_json::Value::String(name.clone()));
    }
    if !rule.formats.is_empty() {
        object.insert("formats".into(), serde_json::json!(rule.formats));
    }
    if !rule.apps.is_empty() {
        object.insert("apps".into(), serde_json::json!(rule.apps));
    }
    if let Some(mode) = rule.app_mode {
        object.insert(
            "app_mode".into(),
            serde_json::to_value(mode).expect("AppMode is serializable"),
        );
    }

    match kind {
        "regexp" => {
            insert_optional(&mut object, "from", rule.from.as_ref());
            insert_optional(&mut object, "to", rule.to.as_ref());
            insert_optional(&mut object, "flags", rule.flags.as_ref());
            insert_optional(&mut object, "message", rule.message.as_ref());
        }
        "url" => {
            if !rule.hosts.is_empty() {
                object.insert("hosts".into(), serde_json::json!(rule.hosts));
            }
            insert_optional(&mut object, "message", rule.message.as_ref());
            if let Some(transform) = &rule.url_transform {
                object.insert(
                    "transform".into(),
                    serde_json::to_value(transform).expect("UrlTransform is serializable"),
                );
            }
        }
        "url-cleanup" => {
            if !rule.hosts.is_empty() {
                object.insert("hosts".into(), serde_json::json!(rule.hosts));
            }
            insert_optional(&mut object, "message", rule.message.as_ref());
            insert_non_empty(
                &mut object,
                "remove_query_params",
                &rule.remove_query_params,
            );
            insert_non_empty(
                &mut object,
                "remove_query_prefixes",
                &rule.remove_query_prefixes,
            );
            insert_non_empty(
                &mut object,
                "remove_query_param_patterns",
                &rule.remove_query_param_patterns,
            );
        }
        "ruleset" => {
            object.insert(
                "mode".into(),
                serde_json::to_value(rule.mode.unwrap_or_default())
                    .expect("RulesetMode is serializable"),
            );
            object.insert(
                "rules".into(),
                serde_json::Value::Array(rule.rules.iter().map(effective_rule_value).collect()),
            );
        }
        _ => {
            if let Some(settings) = &rule.plugin_settings {
                object.extend(settings.clone());
            }
        }
    }
    serde_json::Value::Object(object)
}

fn insert_optional(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<&String>,
) {
    if let Some(value) = value {
        object.insert(key.to_string(), serde_json::Value::String(value.clone()));
    }
}

fn insert_non_empty(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    values: &[String],
) {
    if !values.is_empty() {
        object.insert(key.to_string(), serde_json::json!(values));
    }
}

fn inspect_clipboard() -> Result<()> {
    let paths = ConfigPaths::resolve()?;
    let snapshot_path = paths.state_dir.join("last-clipboard.cbor");
    match ct_runtime::state::LastClipboardSnapshot::load(&snapshot_path) {
        Ok(Some(snapshot)) => {
            let observed_at = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
                snapshot.observed_at_unix_ms as i64,
            )
            .map(|value| value.to_rfc3339())
            .unwrap_or_else(|| snapshot.observed_at_unix_ms.to_string());
            println!("snapshot=stored");
            print_clipboard_item(
                &snapshot.item,
                &observed_at,
                Some(&observed_at),
                snapshot.change_count,
            );
            return Ok(());
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("warning: cannot read stored clipboard snapshot: {error:#}");
            if let Err(error) = ct_runtime::state::quarantine_corrupt_file(&snapshot_path) {
                eprintln!("warning: cannot quarantine stored clipboard snapshot: {error:#}");
            }
        }
    }

    // The platform choice lives inside `ct_clipboard::native`, not here.
    let mut clipboard = native_clipboard_backend()?;

    {
        let change_count = clipboard.change_count()?;
        let observed_at = chrono::Utc::now().to_rfc3339();
        let Some(item) = clipboard.read()? else {
            println!("snapshot=live");
            println!("observed_at={observed_at}");
            println!("copied_at=unavailable");
            print_change_count(change_count);
            println!("clipboard=empty");
            return Ok(());
        };
        println!("snapshot=live");
        print_clipboard_item(&item, &observed_at, None, change_count);
        Ok(())
    }
}

fn print_clipboard_item(
    item: &ClipboardItem,
    observed_at: &str,
    copied_at: Option<&str>,
    change_count: Option<u64>,
) {
    println!("observed_at={observed_at}");
    match copied_at {
        Some(value) => {
            println!("copied_at={value}");
            println!("copied_at.reliability=observed-change");
        }
        None => println!("copied_at=unavailable"),
    }
    print_change_count(change_count);
    if let Some(source) = item.source_app() {
        println!("source_app.name={}", optional_value(source.name.as_deref()));
        println!(
            "source_app.bundle_id={}",
            optional_value(source.bundle_id.as_deref())
        );
        println!("source_app.reliability=best-effort");
    } else {
        println!("source_app=unavailable");
    }
    println!("size_bytes={}", item.size_bytes());
    println!("platform={}", item.platform());
    println!("semantic_views:");
    let mut semantic_count = 0;
    for (alias, value) in [
        ("text", item.text_semantic()),
        ("url", item.url_semantic()),
        ("html", item.html_semantic()),
        ("rtf", item.rtf_semantic()),
    ] {
        let Some(value) = value else {
            continue;
        };
        semantic_count += 1;
        println!(
            "  - alias={alias} authored={} derived_from={} value={}",
            value.is_authored(),
            serde_json::to_string(value.derived_from()).expect("serialize semantic sources"),
            serde_json::to_string(value.value()).expect("serialize semantic value")
        );
    }
    if semantic_count == 0 {
        println!("  (none)");
    }

    println!("native_representations={}", item.representations().len());
    for representation in item.representations() {
        print_representation(representation);
    }
}

fn print_change_count(change_count: Option<u64>) {
    match change_count {
        Some(value) => println!("change_count={value}"),
        None => println!("change_count=unavailable"),
    }
}

fn optional_value(value: Option<&str>) -> &str {
    value.unwrap_or("unavailable")
}

fn print_representation(representation: &ct_clipboard::NativeRepresentation) {
    let bytes = representation.data();
    let descriptor = format!(
        "kind={} id={} flags={} returned_type={} unit_bits={}",
        serde_json::to_string(representation.kind()).expect("serialize native kind"),
        representation
            .id()
            .map(|id| id.to_string())
            .unwrap_or_else(|| "none".into()),
        serde_json::to_string(representation.flags()).expect("serialize native flags"),
        representation.returned_type().unwrap_or("none"),
        representation
            .unit_bits()
            .map(|bits| bits.to_string())
            .unwrap_or_else(|| "none".into())
    );
    if let Ok(text) = std::str::from_utf8(bytes) {
        let preview = serde_json::to_string(text).expect("JSON string serialization cannot fail");
        println!(
            "  - {descriptor} size_bytes={} encoding=utf-8 value={preview}",
            bytes.len()
        );
    } else {
        let preview = bytes
            .iter()
            .take(32)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let suffix = if bytes.len() > 32 { "..." } else { "" };
        println!(
            "  - {descriptor} size_bytes={} encoding=binary hex_preview={preview}{suffix}",
            bytes.len()
        );
    }
}

fn doctor() -> Result<()> {
    let capabilities = ct_runtime::platform::PlatformCapabilities::current();
    println!("platform={}", std::env::consts::OS);
    println!("runtime_available={}", capabilities.runtime_available());
    println!("clipboard={}", capabilities.clipboard);
    println!("notifications={}", capabilities.notifications);
    println!("tray={}", capabilities.tray);
    println!("autostart={}", capabilities.autostart);
    println!("source_app_metadata={}", capabilities.source_app_metadata);
    #[cfg(target_os = "linux")]
    {
        let diagnostics =
            ct_runtime::platform::linux::diagnostics::LinuxSessionDiagnostics::probe();
        println!("session_type={}", diagnostics.session_type);
        println!(
            "desktop={}",
            diagnostics.desktop.as_deref().unwrap_or("unknown")
        );
        println!("session_bus={}", diagnostics.session_bus);
        println!(
            "clipboard_observation={}",
            diagnostics.clipboard_observation
        );
        println!(
            "clipboard_backend={}",
            diagnostics
                .clipboard_backend
                .map_or_else(|| "none".to_string(), |backend| backend.to_string())
        );
        println!("status_notifier_host={}", diagnostics.status_notifier_host);
        println!("desktop_notifications={}", diagnostics.notifications);
        println!(
            "desktop_runtime_ready={}",
            diagnostics.desktop_runtime_ready
        );
        for blocker in diagnostics.blockers {
            println!("desktop_blocker={blocker}");
        }
        println!(
            "support_url={}",
            ct_runtime::platform::linux::diagnostics::SUPPORT_URL
        );
    }
    let paths = ConfigPaths::resolve()?;
    println!("config_file={}", paths.config_file.display());
    Ok(())
}

fn print_schema(
    output: Option<PathBuf>,
    no_plugins: bool,
    only_plugins: Vec<String>,
) -> Result<()> {
    let contributions = if no_plugins {
        Vec::new()
    } else {
        let paths = ConfigPaths::resolve()?;
        let catalog = ct_runtime::plugins::PluginCatalog::discover(&paths.plugins_dir);
        let mut contributions = ct_runtime::config::plugin_schema_contributions(&catalog);
        if !only_plugins.is_empty() {
            for id in &only_plugins {
                let prefix = format!("{id}/");
                if !contributions
                    .iter()
                    .any(|contribution| contribution.rule_type.starts_with(&prefix))
                {
                    eprintln!("warning: plugin {id:?} is not discovered or provides no rule types");
                }
            }
            contributions.retain(|contribution| {
                only_plugins
                    .iter()
                    .any(|id| contribution.rule_type.starts_with(&format!("{id}/")))
            });
        }
        contributions
    };
    let schema = ct_runtime::config::json_schema_pretty_with_plugins(&contributions)?;
    if let Some(output) = output {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::write(&output, schema).with_context(|| format!("write {}", output.display()))?;
        println!("wrote {}", output.display());
    } else {
        println!("{schema}");
    }
    Ok(())
}

fn init_config(config_file: Option<PathBuf>) -> Result<()> {
    let config_path = if let Some(config_file) = config_file {
        config_file
    } else {
        ConfigPaths::resolve()?.config_file
    };
    match ConfigFormat::from_path(&config_path)? {
        ConfigFormat::Yaml => {}
        ConfigFormat::Toml => bail!(
            "init writes the starter YAML config; use a .yaml/.yml path instead of {}",
            config_path.display()
        ),
    }

    let created = ensure_default_config(&config_path)?;
    let schema_path = write_config_schema_next_to(&config_path)?;
    if created {
        println!("created {}", config_path.display());
    } else {
        println!("exists {}", config_path.display());
    }
    println!("wrote {}", schema_path.display());
    Ok(())
}

fn run_plugin_command(command: PluginCommand) -> Result<()> {
    let paths = ConfigPaths::resolve()?;
    match command {
        PluginCommand::Paths => {
            println!("plugins_dir={}", paths.plugins_dir.display());
            Ok(())
        }
        PluginCommand::Install { url } => install_plugin(&paths, &url),
        PluginCommand::Reload => {
            let request_path = paths.state_dir.join("reload-request");
            fs::create_dir_all(&paths.state_dir)
                .with_context(|| format!("create {}", paths.state_dir.display()))?;
            fs::write(&request_path, b"")
                .with_context(|| format!("write {}", request_path.display()))?;
            println!("requested reload via {}", request_path.display());
            Ok(())
        }
        PluginCommand::List => {
            let set = initialize_plugins_for_cli(&paths)?;
            if set.is_empty() {
                println!("no plugins discovered in {}", paths.plugins_dir.display());
                return Ok(());
            }
            for status in set.statuses() {
                let name = status
                    .manifest
                    .as_ref()
                    .map(|manifest| manifest.name.as_str())
                    .unwrap_or("-");
                let version = status
                    .manifest
                    .as_ref()
                    .map(|manifest| manifest.version.as_str())
                    .unwrap_or("-");
                println!(
                    "{} state={} name={:?} version={} rules={} issues={}{}",
                    status.id,
                    status.state.as_str(),
                    name,
                    version,
                    status.available_rules.len(),
                    status.issues.len(),
                    if status.requires_attention() {
                        " attention=required"
                    } else {
                        ""
                    }
                );
            }
            Ok(())
        }
        PluginCommand::Inspect { id } => {
            let set = initialize_plugins_for_cli(&paths)?;
            let status = find_plugin(&set, &id)?;
            println!("id={}", status.id);
            println!("path={}", status.path.display());
            println!("state={}", status.state.as_str());
            if let Some(manifest) = &status.manifest {
                println!("name={}", manifest.name);
                println!("version={}", manifest.version);
                println!("api_version={}", manifest.api_version);
                if let Some(description) = &manifest.description {
                    println!("description={description}");
                }
                if let Some(homepage) = &manifest.homepage {
                    println!("homepage={homepage}");
                }
                println!("requested_capabilities:");
                if manifest.capabilities.is_empty() {
                    println!("  (none)");
                }
                for capability in &manifest.capabilities {
                    println!(
                        "  - kind={} reason={}",
                        capability.kind().as_str(),
                        capability.reason().unwrap_or("-")
                    );
                }
                println!("rule_types:");
                for rule in &manifest.rules {
                    let full = ct_plugin_api::namespaced_rule_type(&manifest.id, &rule.rule_type);
                    let available = status.available_rules.contains(&full);
                    println!(
                        "  - type={full} available={available}{}",
                        rule.description
                            .as_deref()
                            .map(|text| format!(" description={text:?}"))
                            .unwrap_or_default()
                    );
                }
            }
            println!("granted_http_hosts={:?}", status.granted_http_hosts);
            println!("granted_env_expansion={}", status.granted.env_expansion);
            print_plugin_issues(status);
            Ok(())
        }
        PluginCommand::Doctor { id } => {
            let set = initialize_plugins_for_cli(&paths)?;
            let statuses: Vec<_> = match &id {
                Some(id) => vec![find_plugin(&set, id)?],
                None => set.statuses().iter().collect(),
            };
            if statuses.is_empty() {
                println!("no plugins discovered in {}", paths.plugins_dir.display());
                return Ok(());
            }
            for status in statuses {
                println!("plugin={} state={}", status.id, status.state.as_str());
                print_plugin_issues(status);
            }
            Ok(())
        }
        PluginCommand::Example { id } => {
            let set = initialize_plugins_for_cli(&paths)?;
            let status = find_plugin(&set, &id)?;
            let manifest = status
                .manifest
                .as_ref()
                .with_context(|| format!("plugin {id:?} has no readable manifest"))?;
            println!("{}", plugin_config_example(manifest)?);
            Ok(())
        }
    }
}

fn install_plugin(paths: &ConfigPaths, url: &str) -> Result<()> {
    let parsed = url::Url::parse(url).with_context(|| format!("parse plugin URL {url:?}"))?;
    // Plugins are executable code: require an authenticated transport.
    if parsed.scheme() != "https" {
        bail!(
            "plugin install supports only https URLs, got scheme {:?}",
            parsed.scheme()
        );
    }
    paths.ensure_plugins_dir()?;

    // Download into the plugins directory so the final rename stays on one
    // filesystem. Discovery only scans `*.wasm`, so the temp name is
    // invisible to a concurrently running instance.
    let temp_path = paths
        .plugins_dir
        .join(format!(".install-{}.tmp", std::process::id()));
    let result = install_downloaded_plugin(paths, &parsed, &temp_path);
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn install_downloaded_plugin(paths: &ConfigPaths, url: &url::Url, temp_path: &Path) -> Result<()> {
    ct_runtime::platform::download::download_to_file(
        url.as_str(),
        temp_path,
        std::time::Duration::from_secs(60),
        Some(ct_runtime::plugins::MAX_MODULE_BYTES),
    )
    .with_context(|| format!("download plugin {}", url.as_str()))?;
    place_plugin_module(paths, temp_path)
}

/// Validates a downloaded module and moves it to `<plugins_dir>/<id>.wasm`.
fn place_plugin_module(paths: &ConfigPaths, temp_path: &Path) -> Result<()> {
    let metadata = fs::metadata(temp_path)
        .with_context(|| format!("inspect downloaded file {}", temp_path.display()))?;
    if metadata.len() > ct_runtime::plugins::MAX_MODULE_BYTES {
        bail!(
            "downloaded module is {} bytes; the limit is {}",
            metadata.len(),
            ct_runtime::plugins::MAX_MODULE_BYTES
        );
    }
    let module = fs::read(temp_path)
        .with_context(|| format!("read downloaded file {}", temp_path.display()))?;
    let manifest = ct_runtime::plugins::extract_manifest(&module)
        .context("downloaded file is not a valid plugin module")?;

    // Manifest ids cannot contain slashes, so the id is a safe file name.
    let target = paths.plugins_dir.join(format!("{}.wasm", manifest.id));
    for (path, existing) in
        ct_runtime::plugins::PluginCatalog::discover_manifests(&paths.plugins_dir)
    {
        if path == target {
            // Replacing the file at the target path is an upgrade only when
            // it actually contains the same plugin; a renamed module holding
            // a different id must not be destroyed silently.
            if let Ok(existing) = existing.as_ref() {
                if existing.id != manifest.id {
                    bail!(
                        "{} contains plugin id {:?}, not {:?}; remove or rename that file first",
                        path.display(),
                        existing.id,
                        manifest.id
                    );
                }
            }
            continue;
        }
        if existing.is_ok_and(|existing| existing.id == manifest.id) {
            bail!(
                "plugin id {:?} is already provided by {}; remove that file first \
                 (duplicate plugin ids disable every module claiming them)",
                manifest.id,
                path.display()
            );
        }
    }

    let replaced = target.exists();
    fs::rename(temp_path, &target)
        .with_context(|| format!("move plugin into {}", target.display()))?;
    println!(
        "{} {} version {} -> {}",
        if replaced { "updated" } else { "installed" },
        manifest.id,
        manifest.version,
        target.display()
    );
    for rule in &manifest.rules {
        println!(
            "  rule type {}",
            ct_plugin_api::namespaced_rule_type(&manifest.id, &rule.rule_type)
        );
    }
    println!(
        "run `clipboard-transformer plugin inspect {}` for capabilities and issues",
        manifest.id
    );
    Ok(())
}

fn initialize_plugins_for_cli(paths: &ConfigPaths) -> Result<ct_runtime::plugins::PluginSet> {
    let catalog = ct_runtime::plugins::PluginCatalog::discover(&paths.plugins_dir);
    // Permissions and settings come from the config when it loads; a broken
    // config must not make plugin diagnostics unavailable.
    let plugins = resolve_config(None)
        .and_then(|path| {
            ct_runtime::platform::environment::load_dotenv_for_config(&path);
            load_config_with_options(
                path,
                ConfigLoadOptions {
                    state_dir: Some(paths.state_dir.clone()),
                    refresh_url_imports: false,
                    known_rule_types: catalog.known_rule_types(),
                },
            )
        })
        .map(|loaded| loaded.document.plugins)
        .unwrap_or_default();
    Ok(catalog.initialize(&plugins, &ct_runtime::plugins::PluginLimits::default()))
}

fn find_plugin<'a>(
    set: &'a ct_runtime::plugins::PluginSet,
    id: &str,
) -> Result<&'a ct_runtime::plugins::PluginStatus> {
    set.statuses()
        .iter()
        .find(|status| status.id == id)
        .with_context(|| format!("plugin {id:?} is not discovered"))
}

fn print_plugin_issues(status: &ct_runtime::plugins::PluginStatus) {
    println!("issues:");
    if status.issues.is_empty() {
        println!("  (none)");
        return;
    }
    for issue in &status.issues {
        println!(
            "  - code={} severity={:?} attention={:?} summary={:?}",
            issue.code, issue.severity, issue.attention, issue.summary
        );
        if let Some(details) = &issue.details {
            println!("    details={details:?}");
        }
        if let Some(path) = &issue.setting_path {
            println!("    setting_path={path}");
        }
        if !issue.rule_types.is_empty() {
            println!("    rule_types={:?}", issue.rule_types);
        }
    }
}

/// Builds a copyable YAML example from the manifest: the plugin section with
/// requested permissions plus the first example (or a stub) per rule type.
fn plugin_config_example(manifest: &ct_plugin_api::PluginManifest) -> Result<String> {
    use ct_plugin_api::CapabilityKind;

    let mut permissions = serde_yaml::Mapping::new();
    if manifest.requests_capability(CapabilityKind::Http) {
        permissions.insert("http".into(), serde_yaml::to_value(["example.com"])?);
    }
    if manifest.requests_capability(CapabilityKind::EnvExpansion) {
        permissions.insert("env_expansion".into(), true.into());
    }
    let mut plugin_entry = serde_yaml::Mapping::new();
    if !permissions.is_empty() {
        plugin_entry.insert(
            "permissions".into(),
            serde_yaml::Value::Mapping(permissions),
        );
    }
    plugin_entry.insert(
        "settings".into(),
        serde_yaml::Value::Mapping(Default::default()),
    );
    let mut plugins = serde_yaml::Mapping::new();
    plugins.insert(
        manifest.id.clone().into(),
        serde_yaml::Value::Mapping(plugin_entry),
    );

    let mut rules = Vec::new();
    for rule in &manifest.rules {
        let full_type = ct_plugin_api::namespaced_rule_type(&manifest.id, &rule.rule_type);
        let example = rule.examples.first().cloned().unwrap_or_else(|| {
            serde_json::json!({
                "type": full_type,
                "id": format!("{}-example", rule.rule_type),
            })
        });
        let example = serde_yaml::to_value(example)?;
        rules.push(prioritize_rule_example_fields(example));
    }

    let mut document = serde_yaml::Mapping::new();
    document.insert("plugins".into(), serde_yaml::Value::Mapping(plugins));
    document.insert("rules".into(), serde_yaml::Value::Sequence(rules));
    Ok(format_yaml_example(
        &serde_yaml::Value::Mapping(document),
        0,
    ))
}

fn prioritize_rule_example_fields(value: serde_yaml::Value) -> serde_yaml::Value {
    let serde_yaml::Value::Mapping(mut source) = value else {
        return value;
    };
    let mut ordered = serde_yaml::Mapping::new();
    for key in ["type", "id"] {
        let key = serde_yaml::Value::String(key.to_string());
        if let Some(value) = source.remove(&key) {
            ordered.insert(key, value);
        }
    }
    ordered.extend(source);
    serde_yaml::Value::Mapping(ordered)
}

fn format_yaml_example(value: &serde_yaml::Value, indent: usize) -> String {
    match value {
        serde_yaml::Value::Mapping(mapping) => mapping
            .iter()
            .map(|(key, value)| {
                let key = yaml_scalar(key);
                if is_inline_yaml_value(value) {
                    format!(
                        "{}{key}: {}\n",
                        " ".repeat(indent),
                        format_inline_yaml(value)
                    )
                } else {
                    format!(
                        "{}{key}:\n{}",
                        " ".repeat(indent),
                        format_yaml_example(value, indent + 2)
                    )
                }
            })
            .collect(),
        serde_yaml::Value::Sequence(sequence) => sequence
            .iter()
            .map(|value| match value {
                serde_yaml::Value::Mapping(mapping) if !mapping.is_empty() => {
                    let mut entries = mapping.iter();
                    let (key, first_value) = entries.next().expect("mapping is not empty");
                    let mut output = if is_inline_yaml_value(first_value) {
                        format!(
                            "{}- {}: {}\n",
                            " ".repeat(indent),
                            yaml_scalar(key),
                            format_inline_yaml(first_value)
                        )
                    } else {
                        format!(
                            "{}- {}:\n{}",
                            " ".repeat(indent),
                            yaml_scalar(key),
                            format_yaml_example(first_value, indent + 4)
                        )
                    };
                    for (key, value) in entries {
                        if is_inline_yaml_value(value) {
                            output.push_str(&format!(
                                "{}{}: {}\n",
                                " ".repeat(indent + 2),
                                yaml_scalar(key),
                                format_inline_yaml(value)
                            ));
                        } else {
                            output.push_str(&format!(
                                "{}{}:\n{}",
                                " ".repeat(indent + 2),
                                yaml_scalar(key),
                                format_yaml_example(value, indent + 4)
                            ));
                        }
                    }
                    output
                }
                _ if is_inline_yaml_value(value) => {
                    format!("{}- {}\n", " ".repeat(indent), format_inline_yaml(value))
                }
                _ => format!(
                    "{}-\n{}",
                    " ".repeat(indent),
                    format_yaml_example(value, indent + 2)
                ),
            })
            .collect(),
        _ => format!("{}{}\n", " ".repeat(indent), yaml_scalar(value)),
    }
}

fn is_inline_yaml_value(value: &serde_yaml::Value) -> bool {
    match value {
        serde_yaml::Value::Mapping(mapping) => mapping.is_empty(),
        serde_yaml::Value::Sequence(sequence) => sequence.iter().all(is_yaml_scalar),
        value => is_yaml_scalar(value),
    }
}

fn is_yaml_scalar(value: &serde_yaml::Value) -> bool {
    !matches!(
        value,
        serde_yaml::Value::Mapping(_) | serde_yaml::Value::Sequence(_)
    )
}

fn format_inline_yaml(value: &serde_yaml::Value) -> String {
    match value {
        serde_yaml::Value::Mapping(mapping) if mapping.is_empty() => "{}".to_string(),
        serde_yaml::Value::Sequence(sequence) => format!(
            "[{}]",
            sequence
                .iter()
                .map(yaml_scalar)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => yaml_scalar(value),
    }
}

fn yaml_scalar(value: &serde_yaml::Value) -> String {
    serde_yaml::to_string(value)
        .expect("YAML value serializes")
        .trim()
        .trim_start_matches("---\n")
        .to_string()
}

#[cfg(target_os = "macos")]
fn app_executable_path(app: &Path) -> PathBuf {
    app.join("Contents/MacOS").join(APP_EXECUTABLE_NAME)
}

fn resolve_config(config_file: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(config_file) = config_file {
        return Ok(config_file);
    }
    let paths = ConfigPaths::resolve()?;
    if paths.config_file.exists() {
        Ok(paths.config_file)
    } else {
        let toml = paths.config_dir.join("config.toml");
        if toml.exists() {
            Ok(toml)
        } else {
            bail!(
                "config not found; expected {} or {}",
                paths.config_file.display(),
                toml.display()
            )
        }
    }
}

#[cfg(target_os = "macos")]
fn default_app_bundle_path() -> Option<PathBuf> {
    let mut candidates = vec![PathBuf::from("/Applications").join(APP_BUNDLE_NAME)];
    if let Some(home) = home_dir() {
        candidates.push(home.join("Applications").join(APP_BUNDLE_NAME));
    }
    candidates.into_iter().find(|path| path.exists())
}

#[cfg(target_os = "macos")]
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_run_is_not_a_cli_command() {
        let error = Cli::try_parse_from(["clipboard-transformer", "run"]).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn cli_requires_an_explicit_command() {
        let error = Cli::try_parse_from(["clipboard-transformer"]).unwrap_err();
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn transform_accepts_inline_embedding_inputs() {
        let cli = Cli::try_parse_from([
            "clipboard-transformer",
            "transform",
            "-",
            "--config",
            "rules: []",
            "--plugin-dir",
            "/tmp/plugins",
        ])
        .unwrap();
        let CommandKind::Transform {
            source,
            preview,
            input_format,
            inputs,
        } = cli.command
        else {
            panic!("expected transform command");
        };
        assert_eq!(source.as_deref(), Some("-"));
        assert!(!preview);
        assert!(input_format.is_none());
        assert_eq!(inputs.config.as_deref(), Some("rules: []"));
        assert_eq!(
            inputs.plugin_dir.as_deref(),
            Some(Path::new("/tmp/plugins"))
        );
        assert!(inputs.config_file.is_none());
    }

    #[test]
    fn rules_command_accepts_an_explicit_config_file() {
        let cli = Cli::try_parse_from([
            "clipboard-transformer",
            "rules",
            "view",
            "effective",
            "--config-file",
            "/tmp/config.yaml",
        ])
        .unwrap();
        let CommandKind::Rules {
            command:
                RulesCommand::View {
                    inputs,
                    view,
                    format,
                },
        } = cli.command
        else {
            panic!("expected rules view command");
        };
        assert_eq!(
            inputs.config_file.as_deref(),
            Some(Path::new("/tmp/config.yaml"))
        );
        assert_eq!(view, RulesView::Effective);
        assert!(matches!(format, RuleViewFormat::Json));
    }

    #[test]
    fn rules_view_accepts_yaml_and_rejects_unimplemented_views() {
        let cli = Cli::try_parse_from([
            "clipboard-transformer",
            "rules",
            "view",
            "effective",
            "--format",
            "yaml",
        ])
        .unwrap();
        let CommandKind::Rules {
            command: RulesCommand::View { format, .. },
        } = cli.command
        else {
            panic!("expected rules view command");
        };
        assert!(matches!(format, RuleViewFormat::Yaml));

        for view in ["authored", "compiled"] {
            let error =
                Cli::try_parse_from(["clipboard-transformer", "rules", "view", view]).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::InvalidValue);
        }
    }

    #[test]
    fn rules_list_defaults_to_every_known_type_and_can_filter_availability() {
        let all = build_rule_catalog(
            ConfigInputs {
                config: Some("rules: []".to_string()),
                ..ConfigInputs::default()
            },
            false,
        )
        .unwrap();
        let all_types = all
            .rules
            .iter()
            .map(|rule| rule.rule_type.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            all_types,
            [
                "item-shell",
                "regexp",
                "ruleset",
                "shell",
                "url",
                "url-cleanup",
            ]
            .into_iter()
            .collect()
        );
        assert!(
            !all.rules
                .iter()
                .find(|rule| rule.rule_type == "shell")
                .unwrap()
                .available
        );

        let available = build_rule_catalog(
            ConfigInputs {
                config: Some("rules: []".to_string()),
                ..ConfigInputs::default()
            },
            true,
        )
        .unwrap();
        assert!(available.rules.iter().all(|rule| rule.available));
        assert_eq!(available.rules.len(), ct_core::REGISTERED_RULE_TYPES.len());

        let shell_enabled = build_rule_catalog(
            ConfigInputs {
                config: Some("config:\n  shell:\n    enabled: true\nrules: []".to_string()),
                ..ConfigInputs::default()
            },
            true,
        )
        .unwrap();
        assert!(shell_enabled
            .rules
            .iter()
            .any(|rule| rule.rule_type == "shell"));
        assert!(shell_enabled
            .rules
            .iter()
            .any(|rule| rule.rule_type == "item-shell"));
    }

    #[test]
    fn rules_list_includes_manifest_types_from_unavailable_plugins() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("dev.example.demo.wasm"),
            plugin_module("dev.example.demo"),
        )
        .unwrap();
        let inputs = || ConfigInputs {
            config: Some("rules: []".to_string()),
            plugin_dir: Some(dir.path().to_path_buf()),
            ..ConfigInputs::default()
        };

        let all = build_rule_catalog(inputs(), false).unwrap();
        let plugin_rule = all
            .rules
            .iter()
            .find(|rule| rule.rule_type == "dev.example.demo/demo")
            .expect("manifest rule type should be listed");
        assert!(!plugin_rule.available);
        assert_eq!(plugin_rule.source, "plugin:dev.example.demo");

        let available = build_rule_catalog(inputs(), true).unwrap();
        assert!(available
            .rules
            .iter()
            .all(|rule| rule.rule_type != "dev.example.demo/demo"));
    }

    #[test]
    fn builtin_catalog_matches_the_core_registry() {
        let catalog_types = builtin_rule_catalog()
            .into_iter()
            .map(|rule| rule.rule_type)
            .collect::<std::collections::BTreeSet<_>>();
        let registered_types = ct_core::REGISTERED_RULE_TYPES
            .iter()
            .map(|rule_type| (*rule_type).to_string())
            .collect();
        assert_eq!(catalog_types, registered_types);
    }

    #[test]
    fn init_uses_config_file_for_its_output_path() {
        let cli = Cli::try_parse_from([
            "clipboard-transformer",
            "config",
            "init",
            "--config-file",
            "/tmp/config.yaml",
        ])
        .unwrap();
        let CommandKind::Config {
            command: ConfigCommand::Init { config_file },
        } = cli.command
        else {
            panic!("expected config init command");
        };
        assert_eq!(config_file.as_deref(), Some(Path::new("/tmp/config.yaml")));

        let error = Cli::try_parse_from([
            "clipboard-transformer",
            "config",
            "init",
            "--config",
            "/tmp/config.yaml",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn rule_commands_use_config_for_inline_documents() {
        Cli::try_parse_from([
            "clipboard-transformer",
            "config",
            "check",
            "--config",
            "rules: []",
        ])
        .unwrap();
        Cli::try_parse_from([
            "clipboard-transformer",
            "rules",
            "view",
            "effective",
            "--config",
            "rules: []",
        ])
        .unwrap();
        Cli::try_parse_from([
            "clipboard-transformer",
            "transform",
            "-",
            "--config",
            "rules: []",
        ])
        .unwrap();
    }

    #[test]
    fn rule_commands_reject_two_config_sources() {
        let error = Cli::try_parse_from([
            "clipboard-transformer",
            "config",
            "check",
            "--config",
            "rules: []",
            "--config-file",
            "config.yaml",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
        let error = Cli::try_parse_from([
            "clipboard-transformer",
            "rules",
            "view",
            "effective",
            "--config",
            "rules: []",
            "--config-file",
            "config.yaml",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
        let error = Cli::try_parse_from([
            "clipboard-transformer",
            "transform",
            "-",
            "--config",
            "rules: []",
            "--config-file",
            "config.yaml",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn old_ambiguous_command_names_are_rejected() {
        for command in [
            "apply", "validate", "test", "watch", "inspect", "schema", "init",
        ] {
            let error = Cli::try_parse_from(["clipboard-transformer", command]).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
        }
    }

    #[test]
    fn every_visible_command_has_help_text() {
        use clap::CommandFactory;

        fn assert_described(command: &clap::Command) {
            for subcommand in command
                .get_subcommands()
                .filter(|command| !command.is_hide_set())
            {
                assert!(
                    subcommand.get_about().is_some(),
                    "{} is missing help text",
                    subcommand.get_name()
                );
                assert_described(subcommand);
            }
        }

        assert_described(&Cli::command());
    }

    #[test]
    fn top_level_help_orients_terminal_users() {
        use clap::CommandFactory;

        let help = Cli::command().render_long_help().to_string();
        for expected in [
            "optional tool for terminal workflows",
            "config",
            "clipboard",
            "Transform the current clipboard",
            "never starts the desktop app",
            "Examples:",
        ] {
            assert!(help.contains(expected), "help is missing {expected:?}");
        }
    }

    #[test]
    fn inline_config_loads_for_validation_and_effective_rules() {
        let inputs = ConfigInputs {
            config: Some("rules: []".to_string()),
            ..ConfigInputs::default()
        };
        let loaded = load_cli_config(&inputs).unwrap();
        assert!(loaded.config_path.is_none());
        let view = effective_rules_view(&loaded.loaded, None);
        assert!(view["config_path"].is_null());
        assert_eq!(view["rules"], serde_json::json!([]));
        assert!(validate_loaded_config(loaded.loaded, &Default::default())
            .unwrap()
            .is_clean());
    }

    #[test]
    fn effective_rules_view_is_versioned_compact_and_source_aware() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.yaml");
        fs::write(
            &config_path,
            r#"
rules:
  - type: ruleset
    id: clean
    rules:
      - type: url
        id: fragment
        transform:
          type: remove-components
          components: [fragment]
"#,
        )
        .unwrap();

        let loaded = load_config_with_options(&config_path, ConfigLoadOptions::default()).unwrap();
        let view = effective_rules_view(&loaded, Some(&config_path));

        assert_eq!(view["version"], 1);
        assert_eq!(view["view"], "effective");
        assert_eq!(view["rules"][0]["type"], "ruleset");
        assert_eq!(view["rules"][0]["mode"], "all-matching");
        assert_eq!(view["rules"][0]["rules"][0]["type"], "url");
        assert_eq!(
            view["rules"][0]["rules"][0]["transform"]["type"],
            "remove-components"
        );
        assert!(view["rules"][0].get("formats").is_none());
        assert!(
            Path::new(view["rule_sources"]["fragment"]["path"].as_str().unwrap())
                .ends_with("config.yaml")
        );
        assert!(view["rule_sources"]["fragment"]["line"].as_u64().unwrap() > 0);
    }

    #[test]
    fn transform_rejects_two_config_sources() {
        let error = Cli::try_parse_from([
            "clipboard-transformer",
            "transform",
            "-",
            "--config",
            "rules: []",
            "--config-file",
            "config.yaml",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn transform_uses_explicit_dash_for_stdio_and_preview_for_clipboard() {
        let cli = Cli::try_parse_from(["clipboard-transformer", "transform", "-"]).unwrap();
        let CommandKind::Transform {
            source, preview, ..
        } = cli.command
        else {
            panic!("expected transform command");
        };
        assert_eq!(source.as_deref(), Some("-"));
        assert!(!preview);

        let cli = Cli::try_parse_from(["clipboard-transformer", "transform", "--dry-run"]).unwrap();
        let CommandKind::Transform {
            source, preview, ..
        } = cli.command
        else {
            panic!("expected transform command");
        };
        assert!(source.is_none());
        assert!(preview);

        assert!(transform(Some("-"), true, None, ConfigInputs::default())
            .unwrap_err()
            .to_string()
            .contains("--preview is only valid"));
        assert!(
            transform(Some("value"), false, None, ConfigInputs::default())
                .unwrap_err()
                .to_string()
                .contains("use `-` or omit it")
        );
        assert!(transform(None, false, Some("url"), ConfigInputs::default())
            .unwrap_err()
            .to_string()
            .contains("--input-format is only valid"));
    }

    #[test]
    fn inline_transform_engine_applies_rules_without_system_paths() {
        let mut pipeline = load_transform_pipeline(&ConfigInputs {
            config: Some("rules:\n  - id: animals\n    from: cat\n    to: dog\n".to_string()),
            ..ConfigInputs::default()
        })
        .unwrap();

        let result = pipeline
            .engine
            .try_apply(&ClipboardItem::from_text("cat"))
            .unwrap()
            .unwrap();
        assert_eq!(result.after.text(), Some("dog"));
    }

    #[test]
    fn watch_format_is_restricted_to_text_or_jsonl() {
        let error = Cli::try_parse_from([
            "clipboard-transformer",
            "clipboard",
            "watch",
            "--format",
            "json",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::InvalidValue);
    }

    #[test]
    fn watch_transform_defaults_to_both_and_accepts_transformed_only() {
        for (args, expected) in [
            (
                vec!["clipboard-transformer", "clipboard", "watch", "--transform"],
                WatchTransformMode::Both,
            ),
            (
                vec![
                    "clipboard-transformer",
                    "clipboard",
                    "watch",
                    "--transform",
                    "transformed-only",
                ],
                WatchTransformMode::TransformedOnly,
            ),
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            let CommandKind::Clipboard {
                command: ClipboardCommand::Watch { transform, .. },
            } = cli.command
            else {
                panic!("expected clipboard watch command");
            };
            assert_eq!(transform, Some(expected));
        }
    }

    fn test_paths(root: &Path) -> ConfigPaths {
        ConfigPaths {
            config_dir: root.to_path_buf(),
            config_file: root.join("config.yaml"),
            plugins_dir: root.join("plugins"),
            state_dir: root.join("state"),
            cache_dir: root.join("cache"),
        }
    }

    fn plugin_module(id: &str) -> Vec<u8> {
        let manifest = serde_json::json!({
            "id": id,
            "name": "Test",
            "version": "0.1.0",
            "api_version": 1,
            "rules": [{"type": "demo"}],
        });
        build_module_with_sections(&[(
            "clipboard-transformer/manifest",
            serde_json::to_vec(&manifest).unwrap().as_slice(),
        )])
    }

    fn build_module_with_sections(sections: &[(&str, &[u8])]) -> Vec<u8> {
        let mut module = b"\0asm\x01\0\0\0".to_vec();
        for (name, payload) in sections {
            let mut body = Vec::new();
            write_leb128_u32(&mut body, name.len() as u32);
            body.extend_from_slice(name.as_bytes());
            body.extend_from_slice(payload);
            module.push(0);
            write_leb128_u32(&mut module, body.len() as u32);
            module.extend_from_slice(&body);
        }
        module
    }

    fn write_leb128_u32(out: &mut Vec<u8>, mut value: u32) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            out.push(if value == 0 { byte } else { byte | 0x80 });
            if value == 0 {
                break;
            }
        }
    }

    #[test]
    fn place_plugin_module_installs_under_the_manifest_id() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        paths.ensure_plugins_dir().unwrap();
        let temp = paths.plugins_dir.join(".install.tmp");
        fs::write(&temp, plugin_module("dev.example.demo")).unwrap();

        place_plugin_module(&paths, &temp).unwrap();

        assert!(paths.plugins_dir.join("dev.example.demo.wasm").is_file());
        assert!(!temp.exists(), "temp file must be moved, not copied");
    }

    #[test]
    fn place_plugin_module_rejects_invalid_files() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        paths.ensure_plugins_dir().unwrap();
        let temp = paths.plugins_dir.join(".install.tmp");
        fs::write(&temp, b"not wasm at all").unwrap();

        let error = place_plugin_module(&paths, &temp).unwrap_err();

        assert!(
            format!("{error:#}").contains("not a valid plugin module"),
            "{error:#}"
        );
        assert_eq!(
            fs::read_dir(&paths.plugins_dir)
                .unwrap()
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "wasm"))
                .count(),
            0
        );
    }

    #[test]
    fn place_plugin_module_refuses_an_id_owned_by_another_file() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        paths.ensure_plugins_dir().unwrap();
        fs::write(
            paths.plugins_dir.join("other-name.wasm"),
            plugin_module("dev.example.demo"),
        )
        .unwrap();
        let temp = paths.plugins_dir.join(".install.tmp");
        fs::write(&temp, plugin_module("dev.example.demo")).unwrap();

        let error = place_plugin_module(&paths, &temp).unwrap_err();

        assert!(
            format!("{error:#}").contains("already provided by"),
            "{error:#}"
        );
        assert!(!paths.plugins_dir.join("dev.example.demo.wasm").exists());
    }

    #[test]
    fn place_plugin_module_updates_the_same_id_in_place() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        paths.ensure_plugins_dir().unwrap();
        let target = paths.plugins_dir.join("dev.example.demo.wasm");
        fs::write(&target, plugin_module("dev.example.demo")).unwrap();
        let temp = paths.plugins_dir.join(".install.tmp");
        fs::write(&temp, plugin_module("dev.example.demo")).unwrap();

        place_plugin_module(&paths, &temp).unwrap();

        assert!(target.is_file());
    }

    #[test]
    fn clipboard_format_aliases_match_rule_aliases() {
        assert_eq!(
            ct_clipboard::normalize_format("text").unwrap().as_str(),
            "text"
        );
        assert_eq!(
            ct_clipboard::normalize_format("file").unwrap().as_str(),
            "file-url"
        );
        assert_eq!(
            ct_clipboard::normalize_format("custom.binary")
                .unwrap()
                .as_str(),
            "custom.binary"
        );
    }

    #[test]
    fn plugin_examples_put_identity_first_and_format_short_lists_inline() {
        let value = serde_yaml::to_value(serde_json::json!({
            "hosts": ["gitlab.example.com"],
            "id": "example-rule",
            "kinds": ["tree", "blob"],
            "online": true,
            "type": "example.plugin/repository"
        }))
        .unwrap();
        let value = prioritize_rule_example_fields(value);
        let yaml = format_yaml_example(&serde_yaml::Value::Sequence(vec![value]), 2);

        assert_eq!(
            yaml,
            concat!(
                "  - type: example.plugin/repository\n",
                "    id: example-rule\n",
                "    hosts: [gitlab.example.com]\n",
                "    online: true\n",
                "    kinds: [tree, blob]\n",
            )
        );
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        assert!(matches!(parsed, serde_yaml::Value::Sequence(_)));
    }

    #[test]
    fn plugin_example_formatter_indents_nested_mappings_and_sequences() {
        let value = serde_yaml::to_value(serde_json::json!({
            "plugins": {
                "example.plugin": {
                    "permissions": {"http": ["example.com"]},
                    "settings": {}
                }
            }
        }))
        .unwrap();
        let yaml = format_yaml_example(&value, 0);

        assert_eq!(
            yaml,
            concat!(
                "plugins:\n",
                "  example.plugin:\n",
                "    permissions:\n",
                "      http: [example.com]\n",
                "    settings: {}\n",
            )
        );
        assert!(serde_yaml::from_str::<serde_yaml::Value>(&yaml).is_ok());
    }
}
