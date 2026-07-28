use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use ct_core::{
    ClipboardItem, ExternalItemOutput, ExternalItemTransform, ExternalRuleProvider,
    ExternalTextOutput, ExternalTextTransform, ExternalTransform,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::{AppConfig, ConfigPaths, RuleSource};

pub const RULE_TYPES: [&str; 2] = ["shell", "item-shell"];
const NO_MATCH_EXIT: i32 = 3;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ShellHostPaths {
    pub config_file: Option<PathBuf>,
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl ShellHostPaths {
    pub fn from_config_paths(paths: &ConfigPaths, config_file: Option<PathBuf>) -> Self {
        let config_dir = config_file
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| paths.config_dir.clone());
        Self {
            config_file,
            config_dir,
            state_dir: paths.state_dir.clone(),
            cache_dir: paths.cache_dir.clone(),
        }
    }
}

pub fn providers(
    config: &AppConfig,
    paths: ShellHostPaths,
    rule_sources: &BTreeMap<String, RuleSource>,
) -> Vec<Arc<dyn ExternalRuleProvider>> {
    let source_dirs: BTreeMap<String, PathBuf> = rule_sources
        .iter()
        .filter_map(|(id, source)| {
            source
                .path
                .parent()
                .map(|parent| (id.clone(), parent.to_path_buf()))
        })
        .collect();
    vec![
        Arc::new(ShellProvider {
            kind: "shell",
            enabled: config.shell.enabled,
            max_item_bytes: config.max_item_bytes,
            paths: paths.clone(),
            source_dirs: source_dirs.clone(),
        }),
        Arc::new(ShellProvider {
            kind: "item-shell",
            enabled: config.shell.enabled,
            max_item_bytes: config.max_item_bytes,
            paths,
            source_dirs,
        }),
    ]
}

pub fn known_rule_types() -> impl Iterator<Item = String> {
    RULE_TYPES.into_iter().map(str::to_string)
}

struct ShellProvider {
    kind: &'static str,
    enabled: bool,
    max_item_bytes: u64,
    paths: ShellHostPaths,
    source_dirs: BTreeMap<String, PathBuf>,
}

impl ExternalRuleProvider for ShellProvider {
    fn kind(&self) -> &str {
        self.kind
    }

    fn default_formats(&self) -> &[String] {
        &[]
    }

    fn compile(&self, rule_id: &str, settings: &serde_json::Value) -> Result<ExternalTransform> {
        if !self.enabled {
            bail!(
                "native shell rules are disabled; set config.shell.enabled to true to authorize them"
            );
        }
        let mut settings = settings.clone();
        discard_empty_portable_rule_fields(&mut settings);
        let settings: ShellSettings =
            serde_json::from_value(settings).context("invalid shell rule settings")?;
        let source_dir = self
            .source_dirs
            .get(rule_id)
            .cloned()
            .unwrap_or_else(|| self.paths.config_dir.clone());
        let compiled = CompiledShell::new(
            rule_id,
            settings,
            self.max_item_bytes,
            self.paths.clone(),
            source_dir,
        )?;
        match self.kind {
            "shell" => Ok(ExternalTransform::Text(Box::new(ShellTextTransform {
                shell: compiled,
            }))),
            "item-shell" => Ok(ExternalTransform::Item(Box::new(ShellItemTransform {
                shell: compiled,
            }))),
            _ => unreachable!("fixed native shell provider kind"),
        }
    }
}

fn discard_empty_portable_rule_fields(settings: &mut Value) {
    let Some(settings) = settings.as_object_mut() else {
        return;
    };
    for key in ["from", "to", "flags", "message", "mode", "transform"] {
        if settings.get(key).is_some_and(Value::is_null) {
            settings.remove(key);
        }
    }
    for key in [
        "rules",
        "hosts",
        "remove_query_params",
        "remove_query_prefixes",
        "remove_query_param_patterns",
    ] {
        if settings
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        {
            settings.remove(key);
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShellSettings {
    #[serde(default)]
    run: Option<String>,
    #[serde(default)]
    script_path: Option<PathBuf>,
    #[serde(default)]
    shell: Option<String>,
    #[serde(default)]
    timeout: Option<TimeoutValue>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TimeoutValue {
    Seconds(u64),
    Text(String),
}

impl TimeoutValue {
    fn duration(&self) -> Result<Duration> {
        match self {
            Self::Seconds(seconds) => Ok(Duration::from_secs(*seconds)),
            Self::Text(text) => parse_duration(text),
        }
    }
}

fn parse_duration(value: &str) -> Result<Duration> {
    let value = value.trim();
    let (number, multiplier) = if let Some(value) = value.strip_suffix("ms") {
        (value, 1u64)
    } else if let Some(value) = value.strip_suffix('s') {
        (value, 1_000)
    } else if let Some(value) = value.strip_suffix('m') {
        (value, 60_000)
    } else {
        bail!("timeout must be an integer number of seconds or end in ms, s, or m");
    };
    let amount = number
        .trim()
        .parse::<u64>()
        .with_context(|| format!("invalid timeout {value:?}"))?;
    Ok(Duration::from_millis(amount.saturating_mul(multiplier)))
}

struct CompiledShell {
    rule_id: String,
    script: CompiledScript,
    shell: Option<String>,
    timeout: Duration,
    env: BTreeMap<String, String>,
    max_item_bytes: u64,
    paths: ShellHostPaths,
}

enum CompiledScript {
    Inline(String),
    File(PathBuf),
}

impl CompiledShell {
    fn new(
        rule_id: &str,
        settings: ShellSettings,
        max_item_bytes: u64,
        paths: ShellHostPaths,
        source_dir: PathBuf,
    ) -> Result<Self> {
        let script = match (settings.run, settings.script_path) {
            (Some(run), None) => {
                if run.trim().is_empty() {
                    bail!("run must not be empty");
                }
                CompiledScript::Inline(run)
            }
            (None, Some(script_path)) => {
                if script_path.as_os_str().is_empty() {
                    bail!("script_path must not be empty");
                }
                let resolved = if script_path.is_absolute() {
                    script_path
                } else {
                    source_dir.join(script_path)
                };
                let metadata = fs::metadata(&resolved).with_context(|| {
                    format!("read shell script metadata {}", resolved.display())
                })?;
                if !metadata.is_file() {
                    bail!(
                        "shell script path is not a regular file: {}",
                        resolved.display()
                    );
                }
                CompiledScript::File(resolved)
            }
            (Some(_), Some(_)) => bail!("run and script_path are mutually exclusive"),
            (None, None) => bail!("exactly one of run or script_path is required"),
        };
        let timeout = settings
            .timeout
            .as_ref()
            .map(TimeoutValue::duration)
            .transpose()?
            .unwrap_or(DEFAULT_TIMEOUT);
        if timeout.is_zero() {
            bail!("timeout must be greater than zero");
        }
        for key in settings.env.keys() {
            if key.starts_with("CT_") || key == "PWD" {
                bail!("environment variable {key:?} is reserved by the host");
            }
        }
        Ok(Self {
            rule_id: rule_id.to_string(),
            script,
            shell: settings.shell,
            timeout,
            env: settings.env,
            max_item_bytes,
            paths,
        })
    }

    fn run_text(
        &self,
        format: &str,
        input: &str,
        source_app: Option<&ct_core::ClipboardSourceApp>,
    ) -> Result<Option<ExternalTextOutput>> {
        self.run_text_with_paths(&self.paths, format, input, source_app)
    }

    fn run_text_with_paths(
        &self,
        paths: &ShellHostPaths,
        format: &str,
        input: &str,
        source_app: Option<&ct_core::ClipboardSourceApp>,
    ) -> Result<Option<ExternalTextOutput>> {
        let (mut command, temporary_script) = self.command(paths)?;
        command.env("CT_INPUT_FORMAT", format);
        if let Some(bundle_id) = source_app.and_then(|app| app.bundle_id.as_deref()) {
            command.env("CT_SOURCE_APP_BUNDLE_ID", bundle_id);
        }
        if let Some(name) = source_app.and_then(|app| app.name.as_deref()) {
            command.env("CT_SOURCE_APP_NAME", name);
        }
        let output = run_command(command, input.as_bytes(), self.timeout);
        if let Some(path) = temporary_script {
            let _ = fs::remove_file(path);
        }
        let output = output?;
        match output.status {
            ProcessStatus::NoMatch => Ok(None),
            ProcessStatus::Success => {
                let text =
                    String::from_utf8(output.stdout).context("shell stdout is not valid UTF-8")?;
                Ok(Some(ExternalTextOutput {
                    text,
                    message: None,
                }))
            }
            ProcessStatus::Failed(code) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!("shell exited with status {code}: {}", stderr.trim())
            }
            ProcessStatus::TimedOut => bail!("shell timed out after {:?}", self.timeout),
        }
    }

    fn run_item(&self, input: &ClipboardItem) -> Result<Option<ExternalItemOutput>> {
        self.run_item_with_paths(&self.paths, input)
    }

    fn run_item_with_paths(
        &self,
        paths: &ShellHostPaths,
        input: &ClipboardItem,
    ) -> Result<Option<ExternalItemOutput>> {
        let exchange = tempfile::Builder::new()
            .prefix(&format!(
                "clipboard-transformer-shell-{}-",
                encode_rule_id(&self.rule_id)
            ))
            .tempdir()
            .context("create item-shell exchange directory")?;
        let input_dir = exchange.path().join("input");
        let output_dir = exchange.path().join("output");
        fs::create_dir_all(input_dir.join("representations"))?;
        fs::create_dir_all(output_dir.join("representations"))?;
        let input_path = input_dir.join("item.json");
        let output_path = output_dir.join("item.json");
        let wire = item_to_wire(input, &input_dir)?;
        fs::write(&input_path, serde_json::to_vec_pretty(&wire)?)
            .with_context(|| format!("write {}", input_path.display()))?;
        let _input_protection = protect_input_tree(&input_dir)?;

        let (mut command, temporary_script) = self.command(paths)?;
        command
            .env("CT_INPUT_ITEM", &input_path)
            .env("CT_OUTPUT_ITEM", &output_path);
        let output = run_command(command, &[], self.timeout);
        if let Some(path) = temporary_script {
            let _ = fs::remove_file(path);
        }
        let output = output?;
        match output.status {
            ProcessStatus::NoMatch => Ok(None),
            ProcessStatus::Success => {
                if !output_path.is_file() {
                    bail!(
                        "item-shell exited successfully without writing {}",
                        output_path.display()
                    );
                }
                let bytes = fs::read(&output_path)
                    .with_context(|| format!("read {}", output_path.display()))?;
                if bytes.len() > MAX_OUTPUT_BYTES {
                    bail!("item-shell result envelope exceeds {MAX_OUTPUT_BYTES} bytes");
                }
                let envelope: ItemResultEnvelope =
                    serde_json::from_slice(&bytes).context("parse item-shell result envelope")?;
                match envelope {
                    ItemResultEnvelope::NoMatch => Ok(None),
                    ItemResultEnvelope::ReplaceText { text, message } => {
                        let mut item = input.clone();
                        item.replace_with_text(text);
                        Ok(Some(ExternalItemOutput { item, message }))
                    }
                    ItemResultEnvelope::ReplaceItem { item, message } => {
                        let item = wire_to_item(item, &output_dir, self.max_item_bytes)?;
                        Ok(Some(ExternalItemOutput { item, message }))
                    }
                }
            }
            ProcessStatus::Failed(code) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!("item-shell exited with status {code}: {}", stderr.trim())
            }
            ProcessStatus::TimedOut => bail!("item-shell timed out after {:?}", self.timeout),
        }
    }

    fn command(&self, paths: &ShellHostPaths) -> Result<(Command, Option<PathBuf>)> {
        let work_dir = paths
            .cache_dir
            .join("shell")
            .join(encode_rule_id(&self.rule_id));
        fs::create_dir_all(&work_dir)
            .with_context(|| format!("create shell working directory {}", work_dir.display()))?;
        let (script_path, temporary_script) = match &self.script {
            CompiledScript::Inline(run) => {
                let script = tempfile::Builder::new()
                    .prefix("run-")
                    .suffix(script_suffix(self.shell.as_deref()))
                    .tempfile_in(&work_dir)
                    .with_context(|| {
                        format!("create temporary script in {}", work_dir.display())
                    })?;
                let script_path = script.path().to_path_buf();
                fs::write(&script_path, run.as_bytes())
                    .with_context(|| format!("write shell script {}", script_path.display()))?;
                // Persist only for the duration of the child. The runner removes this
                // path after the process opens it; keeping the handle here would drop
                // it before Command::spawn.
                let (_file, persisted_path) =
                    script.keep().context("persist temporary shell script")?;
                (persisted_path.clone(), Some(persisted_path))
            }
            CompiledScript::File(path) => (path.clone(), None),
        };

        let (program, args) = shell_command(self.shell.as_deref(), &script_path)?;
        let mut command = Command::new(&program);
        command
            .args(args)
            .current_dir(&work_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&self.env)
            .env("PWD", &work_dir)
            .env("CT_CONFIG_DIR", &paths.config_dir)
            .env("CT_STATE_DIR", &paths.state_dir)
            .env("CT_CACHE_DIR", &paths.cache_dir)
            .env("CT_RULE_ID", &self.rule_id);
        if let Some(config_file) = &paths.config_file {
            command.env("CT_CONFIG_FILE", config_file);
        }
        Ok((command, temporary_script))
    }
}

struct ShellTextTransform {
    shell: CompiledShell,
}

impl ExternalTextTransform for ShellTextTransform {
    fn transform(
        &mut self,
        format: &str,
        value: &str,
        source_app: Option<&ct_core::ClipboardSourceApp>,
    ) -> Result<Option<ExternalTextOutput>> {
        self.shell.run_text(format, value, source_app)
    }
}

struct ShellItemTransform {
    shell: CompiledShell,
}

impl ExternalItemTransform for ShellItemTransform {
    fn transform(&mut self, item: &ClipboardItem) -> Result<Option<ExternalItemOutput>> {
        self.shell.run_item(item)
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
enum ItemResultEnvelope {
    NoMatch,
    ReplaceText {
        text: String,
        #[serde(default)]
        message: Option<String>,
    },
    ReplaceItem {
        item: Value,
        #[serde(default)]
        message: Option<String>,
    },
}

fn item_to_wire(item: &ClipboardItem, input_dir: &Path) -> Result<Value> {
    let mut value = serde_json::to_value(item).context("serialize clipboard item")?;
    value
        .as_object_mut()
        .context("clipboard item is not an object")?
        .insert("version".into(), json!(1));
    let representations = value
        .get_mut("representations")
        .and_then(Value::as_array_mut)
        .context("clipboard item representations are not an array")?;
    for (index, representation) in representations.iter_mut().enumerate() {
        let object = representation
            .as_object_mut()
            .context("clipboard representation is not an object")?;
        let bytes = json_bytes(
            object
                .remove("data")
                .context("clipboard representation has no data")?,
        )?;
        if let Ok(text) = std::str::from_utf8(&bytes) {
            object.insert("encoding".into(), Value::String("utf8".into()));
            object.insert("data".into(), Value::String(text.to_string()));
        } else {
            let path = input_dir
                .join("representations")
                .join(format!("{index:04}.bin"));
            fs::write(&path, &bytes)
                .with_context(|| format!("write native payload {}", path.display()))?;
            object.insert("encoding".into(), Value::String("file".into()));
            object.insert(
                "path".into(),
                Value::String(path.to_string_lossy().into_owned()),
            );
        }
    }
    Ok(value)
}

fn wire_to_item(mut value: Value, output_dir: &Path, max_item_bytes: u64) -> Result<ClipboardItem> {
    let version = value
        .as_object_mut()
        .and_then(|object| object.remove("version"))
        .and_then(|version| version.as_u64())
        .context("item-shell item has no numeric version")?;
    if version != 1 {
        bail!("unsupported item-shell item version {version}");
    }
    let output_root = output_dir
        .canonicalize()
        .with_context(|| format!("canonicalize {}", output_dir.display()))?;
    let representations = value
        .get_mut("representations")
        .and_then(Value::as_array_mut)
        .context("item-shell representations are not an array")?;
    let mut total = 0usize;
    for representation in representations {
        let object = representation
            .as_object_mut()
            .context("item-shell representation is not an object")?;
        let encoding = object
            .remove("encoding")
            .and_then(|value| value.as_str().map(str::to_string))
            .context("item-shell representation has no encoding")?;
        let bytes = match encoding.as_str() {
            "utf8" => object
                .get("data")
                .and_then(Value::as_str)
                .context("utf8 representation data must be a string")?
                .as_bytes()
                .to_vec(),
            "base64" => base64::engine::general_purpose::STANDARD
                .decode(
                    object
                        .get("data")
                        .and_then(Value::as_str)
                        .context("base64 representation data must be a string")?,
                )
                .context("decode base64 representation")?,
            "file" => {
                let path = PathBuf::from(
                    object
                        .remove("path")
                        .and_then(|value| value.as_str().map(str::to_string))
                        .context("file representation path must be a string")?,
                );
                if !path.is_absolute() {
                    bail!("file representation path must be absolute");
                }
                let metadata = fs::symlink_metadata(&path)
                    .with_context(|| format!("inspect output payload {}", path.display()))?;
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    bail!(
                        "output payload {} must be a regular non-symlink file",
                        path.display()
                    );
                }
                let canonical = path
                    .canonicalize()
                    .with_context(|| format!("canonicalize output payload {}", path.display()))?;
                if !canonical.starts_with(&output_root) {
                    bail!(
                        "output payload {} escapes the output directory",
                        path.display()
                    );
                }
                if max_item_bytes != 0 && u128::from(metadata.len()) > u128::from(max_item_bytes) {
                    bail!("item-shell output payload exceeds max_item_bytes");
                }
                fs::read(&canonical)
                    .with_context(|| format!("read output payload {}", canonical.display()))?
            }
            other => bail!("unsupported item-shell representation encoding {other:?}"),
        };
        total = total.saturating_add(bytes.len());
        if max_item_bytes != 0 && total as u128 > u128::from(max_item_bytes) {
            bail!("item-shell native payloads exceed max_item_bytes");
        }
        object.remove("path");
        object.insert(
            "data".into(),
            Value::Array(bytes.into_iter().map(|byte| json!(byte)).collect()),
        );
    }
    let item: ClipboardItem =
        serde_json::from_value(value).context("decode item-shell ClipboardItem")?;
    if max_item_bytes != 0 && item.size_bytes() as u128 > u128::from(max_item_bytes) {
        bail!("item-shell output exceeds max_item_bytes");
    }
    Ok(item)
}

fn json_bytes(value: Value) -> Result<Vec<u8>> {
    value
        .as_array()
        .context("serialized native payload is not a byte array")?
        .iter()
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| u8::try_from(value).ok())
                .context("serialized native payload contains a non-byte value")
        })
        .collect()
}

struct InputProtection(PathBuf);

impl Drop for InputProtection {
    fn drop(&mut self) {
        let _ = make_input_tree_removable(&self.0);
    }
}

fn protect_input_tree(input_dir: &Path) -> Result<InputProtection> {
    for entry in fs::read_dir(input_dir)? {
        let path = entry?.path();
        if path.is_dir() {
            protect_input_tree(&path)?;
        } else {
            let mut permissions = fs::metadata(&path)?.permissions();
            permissions.set_readonly(true);
            fs::set_permissions(&path, permissions)?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(input_dir, fs::Permissions::from_mode(0o555))?;
    }
    Ok(InputProtection(input_dir.to_path_buf()))
}

#[cfg(windows)]
#[allow(clippy::permissions_set_readonly_false)]
fn clear_windows_readonly(path: &Path) -> Result<()> {
    // Windows models read-only as a file attribute, so clearing it does not
    // introduce the Unix world-writable behavior guarded by this Clippy lint.
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

fn make_input_tree_removable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path.is_dir() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
        } else {
            fs::set_permissions(path, fs::Permissions::from_mode(0o644))?;
        }
    }
    #[cfg(windows)]
    if path.is_file() {
        clear_windows_readonly(path)?;
    }
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            make_input_tree_removable(&entry?.path())?;
        }
    }
    Ok(())
}

enum ProcessStatus {
    Success,
    NoMatch,
    Failed(i32),
    TimedOut,
}

struct ProcessOutput {
    status: ProcessStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_command(mut command: Command, input: &[u8], timeout: Duration) -> Result<ProcessOutput> {
    configure_process_group(&mut command);
    let mut child = command.spawn().context("start shell process")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input).context("write shell stdin")?;
    }
    let stdout = child.stdout.take().context("capture shell stdout")?;
    let stderr = child.stderr.take().context("capture shell stderr")?;
    let stdout_thread = thread::spawn(move || read_bounded(stdout, MAX_OUTPUT_BYTES));
    let stderr_thread = thread::spawn(move || read_bounded(stderr, MAX_OUTPUT_BYTES));

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().context("poll shell process")? {
            break match status.code() {
                Some(0) => ProcessStatus::Success,
                Some(NO_MATCH_EXIT) => ProcessStatus::NoMatch,
                Some(code) => ProcessStatus::Failed(code),
                None => ProcessStatus::Failed(-1),
            };
        }
        if Instant::now() >= deadline {
            terminate_process_tree(&mut child).context("terminate timed out shell process")?;
            let _ = child.wait();
            break ProcessStatus::TimedOut;
        }
        thread::sleep(Duration::from_millis(10));
    };

    let stdout = stdout_thread
        .join()
        .map_err(|_| anyhow!("shell stdout reader panicked"))??;
    let stderr = stderr_thread
        .join()
        .map_err(|_| anyhow!("shell stderr reader panicked"))??;
    Ok(ProcessOutput {
        status,
        stdout,
        stderr,
    })
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: setpgid is async-signal-safe and touches only the child process
    // between fork and exec. A new group lets timeout cleanup include every
    // process spawned by the script.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
}

#[cfg(windows)]
fn configure_process_group(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

#[cfg(unix)]
fn terminate_process_tree(child: &mut std::process::Child) -> Result<()> {
    let process_group = i32::try_from(child.id()).context("child pid exceeds i32")?;
    // SAFETY: the negative pid addresses the process group created immediately
    // before spawn. SIGKILL is used only after the configured timeout.
    let result = unsafe { libc::kill(-process_group, libc::SIGKILL) };
    if result == 0 {
        Ok(())
    } else {
        child
            .kill()
            .context("kill shell child after group kill failed")
    }
}

#[cfg(windows)]
fn terminate_process_tree(child: &mut std::process::Child) -> Result<()> {
    let status = Command::new("taskkill")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .status();
    if status.is_ok_and(|status| status.success()) {
        Ok(())
    } else {
        child
            .kill()
            .context("kill shell child after taskkill failed")
    }
}

fn read_bounded(mut reader: impl Read, limit: usize) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut oversized = false;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(remaining)]);
        oversized |= count > remaining;
    }
    if oversized {
        bail!("shell output exceeds {limit} bytes");
    }
    Ok(output)
}

fn shell_command(shell: Option<&str>, script: &Path) -> Result<(OsString, Vec<OsString>)> {
    let shell = shell.map(str::trim).filter(|shell| !shell.is_empty());
    if let Some(template) = shell.filter(|shell| shell.contains("{0}")) {
        let parts = shlex::split(template).context("invalid shell command template")?;
        let (program, args) = parts
            .split_first()
            .context("shell command template is empty")?;
        let script = script.as_os_str().to_string_lossy();
        return Ok((
            OsString::from(program),
            args.iter()
                .map(|arg| OsString::from(arg.replace("{0}", &script)))
                .collect(),
        ));
    }

    let program = shell
        .map(OsString::from)
        .unwrap_or_else(system_default_shell);
    let name = Path::new(&program)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let args = match name.as_str() {
        "pwsh" | "powershell" => vec![
            "-NoLogo".into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-File".into(),
            script.as_os_str().to_owned(),
        ],
        "cmd" => vec![
            "/D".into(),
            "/S".into(),
            "/C".into(),
            script.as_os_str().to_owned(),
        ],
        _ => vec![script.as_os_str().to_owned()],
    };
    Ok((program, args))
}

fn system_default_shell() -> OsString {
    #[cfg(windows)]
    {
        std::env::var_os("COMSPEC").unwrap_or_else(|| OsString::from("cmd.exe"))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"))
    }
}

fn script_suffix(shell: Option<&str>) -> &'static str {
    #[cfg(windows)]
    if shell.is_none() {
        return ".cmd";
    }
    let shell = shell.unwrap_or_default().to_ascii_lowercase();
    if shell.contains("pwsh") || shell.contains("powershell") {
        ".ps1"
    } else if shell == "cmd" || shell.ends_with("cmd.exe") {
        ".cmd"
    } else {
        ".sh"
    }
}

fn encode_rule_id(rule_id: &str) -> String {
    let mut encoded = String::new();
    for byte in rule_id.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_paths_for_any_platform(root: &Path) -> ShellHostPaths {
        ShellHostPaths {
            config_dir: root.join("config"),
            config_file: Some(root.join("config/config.yaml")),
            state_dir: root.join("state"),
            cache_dir: root.join("cache"),
        }
    }

    #[cfg(unix)]
    fn compiled(run: &str) -> CompiledShell {
        CompiledShell::new(
            "test-shell",
            ShellSettings {
                run: Some(run.to_string()),
                script_path: None,
                shell: Some("/bin/sh".to_string()),
                timeout: Some(TimeoutValue::Text("2s".to_string())),
                env: BTreeMap::new(),
            },
            100 * 1024 * 1024,
            test_paths_for_any_platform(Path::new("/tmp/clipboard-transformer-shell-test")),
            PathBuf::from("/tmp/clipboard-transformer-shell-test/config"),
        )
        .unwrap()
    }

    #[test]
    fn timeout_parser_accepts_documented_units() {
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
        assert_eq!(parse_duration("2s").unwrap(), Duration::from_secs(2));
        assert_eq!(parse_duration("3m").unwrap(), Duration::from_secs(180));
    }

    #[test]
    fn rule_id_encoding_is_stable_and_readable() {
        assert_eq!(encode_rule_id("google-link"), "google-link");
        assert_eq!(encode_rule_id("google/docs"), "google%2Fdocs");
    }

    #[test]
    fn script_source_requires_exactly_one_form() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths_for_any_platform(root.path());
        let source_dir = root.path().join("config");
        let missing: ShellSettings = serde_json::from_value(json!({})).unwrap();
        let error = CompiledShell::new("missing", missing, 1024, paths.clone(), source_dir.clone())
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("exactly one"));

        let both: ShellSettings = serde_json::from_value(json!({
            "run": "echo inline",
            "script_path": "script.sh"
        }))
        .unwrap();
        let error = CompiledShell::new("both", both, 1024, paths, source_dir)
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("mutually exclusive"));
    }

    #[cfg(unix)]
    #[test]
    fn text_shell_uses_stdin_stdout_and_no_match_exit() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths_for_any_platform(root.path());
        let output = compiled("tr '[:lower:]' '[:upper:]'")
            .run_text_with_paths(&paths, "text", "Hello", None)
            .unwrap()
            .unwrap();
        assert_eq!(output.text, "HELLO");

        let no_match = compiled("exit 3")
            .run_text_with_paths(&paths, "text", "Hello", None)
            .unwrap();
        assert!(no_match.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn script_path_resolves_from_the_declaring_config_directory() {
        let root = tempfile::tempdir().unwrap();
        let source_dir = root.path().join("rules");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(
            source_dir.join("uppercase.sh"),
            "tr '[:lower:]' '[:upper:]'",
        )
        .unwrap();
        let shell = CompiledShell::new(
            "file-shell",
            ShellSettings {
                run: None,
                script_path: Some(PathBuf::from("uppercase.sh")),
                shell: Some("/bin/sh".to_string()),
                timeout: None,
                env: BTreeMap::new(),
            },
            1024,
            test_paths_for_any_platform(root.path()),
            source_dir,
        )
        .unwrap();

        let output = shell.run_text("text", "from file", None).unwrap().unwrap();
        assert_eq!(output.text, "FROM FILE");
    }

    #[cfg(unix)]
    #[test]
    fn item_shell_can_return_replace_text() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths_for_any_platform(root.path());
        let output = compiled(
            r#"printf '%s' '{"action":"replace-text","text":"formatted"}' > "$CT_OUTPUT_ITEM""#,
        )
        .run_item_with_paths(&paths, &ClipboardItem::from_text("original"))
        .unwrap()
        .unwrap();
        assert_eq!(output.item.text(), Some("formatted"));
    }

    #[test]
    fn item_wire_uses_utf8_without_base64() {
        let root = tempfile::tempdir().unwrap();
        let mut item = ClipboardItem::new();
        item.set_native(ct_core::NativeRepresentation::named(
            "text/plain",
            b"hello".to_vec(),
        ));
        item.set_derived_text("hello", vec!["text/plain".to_string()]);
        let wire = item_to_wire(&item, root.path()).unwrap();
        assert_eq!(
            wire.pointer("/semantics/text/value")
                .and_then(Value::as_str),
            Some("hello")
        );
        assert_eq!(
            wire.pointer("/representations/0/encoding")
                .and_then(Value::as_str),
            Some("utf8")
        );
    }

    #[test]
    fn item_wire_round_trips_binary_output_from_an_absolute_temp_file() {
        let root = tempfile::tempdir().unwrap();
        let input_dir = root.path().join("input");
        let output_dir = root.path().join("output");
        fs::create_dir_all(input_dir.join("representations")).unwrap();
        fs::create_dir_all(output_dir.join("representations")).unwrap();
        let mut item = ClipboardItem::new();
        item.set_native(ct_core::NativeRepresentation::named(
            "application/octet-stream",
            vec![0, 159, 255],
        ));
        let mut wire = item_to_wire(&item, &input_dir).unwrap();
        let output_payload = output_dir.join("representations/0000.bin");
        fs::write(&output_payload, [0, 159, 255]).unwrap();
        let representation = wire
            .pointer_mut("/representations/0")
            .and_then(Value::as_object_mut)
            .unwrap();
        representation.insert(
            "path".into(),
            Value::String(output_payload.to_string_lossy().into_owned()),
        );

        let decoded = wire_to_item(wire, &output_dir, 1024).unwrap();
        assert_eq!(
            decoded
                .representation("application/octet-stream")
                .unwrap()
                .data(),
            [0, 159, 255]
        );
    }

    #[test]
    fn item_wire_rejects_output_files_outside_the_exchange_directory() {
        let root = tempfile::tempdir().unwrap();
        let input_dir = root.path().join("input");
        let output_dir = root.path().join("output");
        fs::create_dir_all(input_dir.join("representations")).unwrap();
        fs::create_dir_all(&output_dir).unwrap();
        let outside = root.path().join("outside.bin");
        fs::write(&outside, [1, 2, 3]).unwrap();
        let mut item = ClipboardItem::new();
        item.set_native(ct_core::NativeRepresentation::named(
            "application/octet-stream",
            vec![0, 159, 255],
        ));
        let mut wire = item_to_wire(&item, &input_dir).unwrap();
        wire.pointer_mut("/representations/0")
            .and_then(Value::as_object_mut)
            .unwrap()
            .insert(
                "path".into(),
                Value::String(outside.to_string_lossy().into_owned()),
            );

        let error = wire_to_item(wire, &output_dir, 1024).unwrap_err();
        assert!(format!("{error:#}").contains("escapes the output directory"));
    }

    #[test]
    fn item_wire_accepts_small_inline_base64_output() {
        let root = tempfile::tempdir().unwrap();
        let output_dir = root.path().join("output");
        fs::create_dir_all(&output_dir).unwrap();
        let wire = json!({
            "version": 1,
            "platform": "portable",
            "representations": [{
                "kind": "application/octet-stream",
                "encoding": "base64",
                "data": "AJ//"
            }],
            "semantics": {},
            "source_app": null
        });

        let decoded = wire_to_item(wire, &output_dir, 1024).unwrap();
        assert_eq!(
            decoded
                .representation("application/octet-stream")
                .unwrap()
                .data(),
            [0, 159, 255]
        );
    }
}
