//! Effective environment for desktop launches.
//!
//! A Unix GUI process can start with a much smaller launchd/session
//! environment than the user's non-interactive login shell. The bootstrap in
//! this module runs before path resolution, adds only missing login-shell
//! values to the process, and keeps an immutable copy of the original GUI
//! environment so later refreshes preserve its precedence.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

static DOTENV: OnceLock<RwLock<DotenvSnapshot>> = OnceLock::new();
static ENVIRONMENT_REVISION: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
struct DotenvSnapshot {
    path: PathBuf,
    values: BTreeMap<OsString, OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DotenvReport {
    pub path: PathBuf,
    pub present: bool,
    pub loaded_count: usize,
    pub ignored_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentRefreshMode {
    Background,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentRefreshReport {
    pub revision: u64,
    pub dotenv: DotenvReport,
    pub dotenv_changed: bool,
    pub shell_warning: Option<String>,
}

#[derive(Debug, Default)]
struct DotenvRead {
    values: BTreeMap<OsString, OsString>,
    ignored_count: usize,
}

#[cfg(unix)]
mod unix {
    use std::ffi::CStr;
    use std::fs::{self, File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::{OnceLock, RwLock};
    use std::thread;
    use std::time::{Duration, Instant};

    use anyhow::{anyhow, bail, Context, Result};

    use super::{BTreeMap, OsStr, OsString};

    const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
    const CAPTURE_POLL_INTERVAL: Duration = Duration::from_millis(10);
    const MAX_CAPTURE_BYTES: u64 = 1024 * 1024;
    const DEFAULT_PASSWD_BUFFER_BYTES: usize = 16 * 1024;
    const MAX_PASSWD_BUFFER_BYTES: usize = 1024 * 1024;

    static GUI_ENVIRONMENT: OnceLock<RwLock<GuiEnvironment>> = OnceLock::new();

    #[derive(Debug)]
    struct GuiEnvironment {
        original_process: BTreeMap<OsString, OsString>,
        effective: BTreeMap<OsString, OsString>,
        shell: PathBuf,
    }

    #[derive(Debug, Clone)]
    pub struct BootstrapReport {
        pub shell: Option<PathBuf>,
        pub imported_count: usize,
        pub warning: Option<String>,
    }

    pub fn bootstrap_gui_environment() -> BootstrapReport {
        let original_process = std::env::vars_os().collect::<BTreeMap<_, _>>();
        let shell = default_login_shell(&original_process);
        let (effective, shell, warning) = match shell {
            Ok(shell) => match capture_login_environment(
                &shell,
                &original_process,
                CAPTURE_TIMEOUT,
                MAX_CAPTURE_BYTES,
            ) {
                Ok(login) => (
                    merge_environment(login, &original_process),
                    Some(shell),
                    None,
                ),
                Err(error) => (
                    original_process.clone(),
                    Some(shell),
                    Some(format!("login-shell environment unavailable: {error:#}")),
                ),
            },
            Err(error) => (
                original_process.clone(),
                None,
                Some(format!("default login shell unavailable: {error:#}")),
            ),
        };

        let imported = effective
            .iter()
            .filter(|(name, _)| !original_process.contains_key(*name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<Vec<_>>();

        // This is called before config resolution and before worker threads are
        // started. Later refreshes update only the snapshot used by host
        // lookups and child commands; they never mutate the process environment.
        for (name, value) in &imported {
            std::env::set_var(name, value);
        }

        let stored_shell = shell.clone().unwrap_or_else(|| PathBuf::from("/bin/sh"));
        let state = GuiEnvironment {
            original_process,
            effective,
            shell: stored_shell,
        };
        if let Some(existing) = GUI_ENVIRONMENT.get() {
            if let Ok(mut existing) = existing.write() {
                *existing = state;
            }
        } else {
            let _ = GUI_ENVIRONMENT.set(RwLock::new(state));
        }

        BootstrapReport {
            shell,
            imported_count: imported.len(),
            warning,
        }
    }

    pub fn refresh_gui_environment() -> Result<bool> {
        let Some(environment) = GUI_ENVIRONMENT.get() else {
            return Ok(false);
        };
        let mut environment = environment
            .write()
            .map_err(|_| anyhow!("GUI environment lock poisoned"))?;
        let shell = default_login_shell(&environment.original_process)
            .unwrap_or_else(|_| environment.shell.clone());
        let login = capture_login_environment(
            &shell,
            &environment.original_process,
            CAPTURE_TIMEOUT,
            MAX_CAPTURE_BYTES,
        )?;
        let effective = merge_environment(login, &environment.original_process);
        let changed = effective != environment.effective;
        environment.effective = effective;
        environment.shell = shell;
        Ok(changed)
    }

    pub fn is_active() -> bool {
        GUI_ENVIRONMENT.get().is_some()
    }

    pub fn var_with_dotenv(name: &str, dotenv: &BTreeMap<OsString, OsString>) -> Option<String> {
        let environment = GUI_ENVIRONMENT.get()?.read().ok()?;
        let name = OsStr::new(name);
        layered_value(
            name,
            &environment.original_process,
            dotenv,
            &environment.effective,
        )
        .and_then(|value| value.to_str())
        .map(str::to_owned)
    }

    pub fn command_environment(
        dotenv: &BTreeMap<OsString, OsString>,
    ) -> Option<BTreeMap<OsString, OsString>> {
        let environment = GUI_ENVIRONMENT.get()?.read().ok()?;
        let mut effective = environment.effective.clone();
        effective.extend(
            dotenv
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        );
        effective.extend(
            environment
                .original_process
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        );
        Some(effective)
    }

    fn layered_value<'a>(
        name: &OsStr,
        process: &'a BTreeMap<OsString, OsString>,
        dotenv: &'a BTreeMap<OsString, OsString>,
        login: &'a BTreeMap<OsString, OsString>,
    ) -> Option<&'a OsString> {
        process
            .get(name)
            .or_else(|| dotenv.get(name))
            .or_else(|| login.get(name))
    }

    fn merge_environment(
        mut lower_priority: BTreeMap<OsString, OsString>,
        process: &BTreeMap<OsString, OsString>,
    ) -> BTreeMap<OsString, OsString> {
        lower_priority.extend(
            process
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        );
        lower_priority
    }

    fn default_login_shell(process: &BTreeMap<OsString, OsString>) -> Result<PathBuf> {
        match passwd_login_shell() {
            Ok(shell) => Ok(shell),
            Err(passwd_error) => process
                .get(OsStr::new("SHELL"))
                .filter(|shell| !shell.is_empty())
                .map(PathBuf::from)
                .ok_or(passwd_error),
        }
    }

    fn passwd_login_shell() -> Result<PathBuf> {
        let requested = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
        let buffer_len = if requested <= 0 {
            DEFAULT_PASSWD_BUFFER_BYTES
        } else {
            usize::try_from(requested)
                .unwrap_or(DEFAULT_PASSWD_BUFFER_BYTES)
                .min(MAX_PASSWD_BUFFER_BYTES)
        };
        let mut buffer = vec![0_u8; buffer_len];
        let mut passwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let status = unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                passwd.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status != 0 {
            return Err(std::io::Error::from_raw_os_error(status)).context("resolve login user");
        }
        if result.is_null() {
            bail!("login user is missing from the account database");
        }
        let passwd = unsafe { passwd.assume_init() };
        if passwd.pw_shell.is_null() {
            bail!("login user has no default shell");
        }
        let shell = unsafe { CStr::from_ptr(passwd.pw_shell) }.to_bytes();
        if shell.is_empty() {
            bail!("login user has an empty default shell");
        }
        Ok(PathBuf::from(OsString::from_vec(shell.to_vec())))
    }

    fn capture_login_environment(
        shell: &Path,
        process: &BTreeMap<OsString, OsString>,
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<BTreeMap<OsString, OsString>> {
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let start_marker = format!("clipboard_transformer_env_start_{nonce}");
        let end_marker = format!("clipboard_transformer_env_end_{nonce}");
        let executable = std::env::current_exe().context("resolve current executable")?;
        let mut script = shell_quote(executable.as_os_str());
        script.push(" __dump-environment ");
        script.push(shell_quote(OsStr::new(&nonce)));
        capture_shell_environment(
            shell,
            process,
            timeout,
            max_bytes,
            &script,
            start_marker.as_bytes(),
            end_marker.as_bytes(),
        )
    }

    fn capture_shell_environment(
        shell: &Path,
        process: &BTreeMap<OsString, OsString>,
        timeout: Duration,
        max_bytes: u64,
        script: &OsStr,
        start_marker: &[u8],
        end_marker: &[u8],
    ) -> Result<BTreeMap<OsString, OsString>> {
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let capture_path = std::env::temp_dir().join(format!(
            "clipboard-transformer-env-{}-{nonce}",
            std::process::id()
        ));
        let capture = CaptureFile::create(capture_path)?;
        let stdout = capture
            .file
            .try_clone()
            .context("clone environment capture file")?;
        let shell_name = shell
            .file_name()
            .filter(|name| !name.is_empty())
            .context("login shell path has no file name")?;
        let mut login_argv0 = OsString::from("-");
        login_argv0.push(shell_name);
        let mut command = Command::new(shell);
        command
            // The leading '-' in argv[0] is the standard Unix login-shell
            // convention. `-l` also makes non-interactive bash load its login
            // profile, while `-c` keeps the shell non-interactive. For zsh
            // this reads .zshenv, .zprofile and .zlogin, but not .zshrc.
            .arg0(login_argv0)
            .arg("-l")
            .arg("-c")
            .arg(script)
            .env_clear()
            .envs(process)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .with_context(|| format!("launch login shell {}", shell.display()))?;

        let deadline = Instant::now() + timeout;
        let status = loop {
            if capture.len().unwrap_or(max_bytes + 1) > max_bytes {
                let _ = child.kill();
                let _ = child.wait();
                bail!("login-shell environment output exceeded {max_bytes} bytes");
            }
            if let Some(status) = child.try_wait().context("wait for login shell")? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                bail!("login shell timed out after {} ms", timeout.as_millis());
            }
            thread::sleep(CAPTURE_POLL_INTERVAL);
        };
        if !status.success() {
            bail!("login shell exited with {status}");
        }

        let bytes = capture.read(max_bytes)?;
        parse_capture(&bytes, start_marker, end_marker)
    }

    fn shell_quote(value: &OsStr) -> OsString {
        let bytes = value.as_bytes();
        let mut quoted = Vec::with_capacity(bytes.len() + 2);
        quoted.push(b'\'');
        for byte in bytes {
            if *byte == b'\'' {
                quoted.extend_from_slice(b"'\"'\"'");
            } else {
                quoted.push(*byte);
            }
        }
        quoted.push(b'\'');
        OsString::from_vec(quoted)
    }

    pub fn dump_current_environment(marker: &str) -> std::io::Result<()> {
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        write_environment_dump(&mut stdout, marker, std::env::vars_os())
    }

    fn write_environment_dump<W, I>(
        writer: &mut W,
        marker: &str,
        environment: I,
    ) -> std::io::Result<()>
    where
        W: Write,
        I: IntoIterator<Item = (OsString, OsString)>,
    {
        writer.write_all(b"\0clipboard_transformer_env_start_")?;
        writer.write_all(marker.as_bytes())?;
        writer.write_all(b"\0")?;
        for (name, value) in environment {
            writer.write_all(name.as_bytes())?;
            writer.write_all(b"=")?;
            writer.write_all(value.as_bytes())?;
            writer.write_all(b"\0")?;
        }
        writer.write_all(b"\0clipboard_transformer_env_end_")?;
        writer.write_all(marker.as_bytes())?;
        writer.write_all(b"\0")?;
        writer.flush()
    }

    fn parse_capture(
        output: &[u8],
        start_marker: &[u8],
        end_marker: &[u8],
    ) -> Result<BTreeMap<OsString, OsString>> {
        let start = nul_wrapped(start_marker);
        let end = nul_wrapped(end_marker);
        let body_start = find_bytes(output, &start)
            .map(|index| index + start.len())
            .context("login-shell environment start marker missing")?;
        let body_end = find_bytes(&output[body_start..], &end)
            .map(|index| body_start + index)
            .context("login-shell environment end marker missing")?;
        let mut environment = BTreeMap::new();
        for entry in output[body_start..body_end]
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
        {
            let Some(separator) = entry.iter().position(|byte| *byte == b'=') else {
                continue;
            };
            if separator == 0 {
                continue;
            }
            environment.insert(
                OsString::from_vec(entry[..separator].to_vec()),
                OsString::from_vec(entry[separator + 1..].to_vec()),
            );
        }
        Ok(environment)
    }

    fn nul_wrapped(marker: &[u8]) -> Vec<u8> {
        let mut wrapped = Vec::with_capacity(marker.len() + 2);
        wrapped.push(0);
        wrapped.extend_from_slice(marker);
        wrapped.push(0);
        wrapped
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    struct CaptureFile {
        path: PathBuf,
        file: File,
    }

    impl CaptureFile {
        fn create(path: PathBuf) -> Result<Self> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .with_context(|| format!("create environment capture {}", path.display()))?;
            Ok(Self { path, file })
        }

        fn len(&self) -> Result<u64> {
            Ok(self.file.metadata()?.len())
        }

        fn read(&self, max_bytes: u64) -> Result<Vec<u8>> {
            let file = File::open(&self.path)
                .with_context(|| format!("open environment capture {}", self.path.display()))?;
            let mut bytes = Vec::new();
            file.take(max_bytes + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > max_bytes {
                bail!("login-shell environment output exceeded {max_bytes} bytes");
            }
            Ok(bytes)
        }
    }

    impl Drop for CaptureFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn os_map(items: &[(&str, &str)]) -> BTreeMap<OsString, OsString> {
            items
                .iter()
                .map(|(name, value)| (OsString::from(name), OsString::from(value)))
                .collect()
        }

        #[test]
        fn process_environment_has_precedence_over_login_shell() {
            let merged = merge_environment(
                os_map(&[("LOGIN_ONLY", "login"), ("BOTH", "login")]),
                &os_map(&[("GUI_ONLY", "gui"), ("BOTH", "gui")]),
            );
            assert_eq!(merged.get(OsStr::new("LOGIN_ONLY")), Some(&"login".into()));
            assert_eq!(merged.get(OsStr::new("GUI_ONLY")), Some(&"gui".into()));
            assert_eq!(merged.get(OsStr::new("BOTH")), Some(&"gui".into()));
        }

        #[test]
        fn dotenv_sits_between_process_and_login_shell() {
            let process = os_map(&[("PROCESS", "process"), ("ALL", "process")]);
            let dotenv = os_map(&[("DOTENV", "dotenv"), ("ALL", "dotenv"), ("LOW", "dotenv")]);
            let login = os_map(&[("LOGIN", "login"), ("ALL", "login"), ("LOW", "login")]);

            assert_eq!(
                layered_value(OsStr::new("ALL"), &process, &dotenv, &login),
                Some(&"process".into())
            );
            assert_eq!(
                layered_value(OsStr::new("LOW"), &process, &dotenv, &login),
                Some(&"dotenv".into())
            );
            assert_eq!(
                layered_value(OsStr::new("LOGIN"), &process, &dotenv, &login),
                Some(&"login".into())
            );
        }

        #[test]
        fn capture_parser_ignores_shell_output_outside_markers() {
            let output = b"startup noise\0start\0A=one\0B=two=three\0\0end\0logout noise";
            let parsed = parse_capture(output, b"start", b"end").unwrap();
            assert_eq!(parsed, os_map(&[("A", "one"), ("B", "two=three")]));
        }

        #[test]
        fn environment_dump_is_nul_delimited_and_framed() {
            let mut output = Vec::new();
            write_environment_dump(
                &mut output,
                "test",
                os_map(&[("A", "one"), ("B", "two=three")]),
            )
            .unwrap();

            let parsed = parse_capture(
                &output,
                b"clipboard_transformer_env_start_test",
                b"clipboard_transformer_env_end_test",
            )
            .unwrap();
            assert_eq!(parsed, os_map(&[("A", "one"), ("B", "two=three")]));
        }

        #[test]
        fn shell_quote_handles_spaces_and_single_quotes() {
            assert_eq!(
                shell_quote(OsStr::new("/tmp/it's here")),
                OsString::from("'/tmp/it'\"'\"'s here'")
            );
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn bash_capture_is_login_but_not_interactive() {
            let dir = tempfile::tempdir().unwrap();
            fs::write(
                dir.path().join(".bash_profile"),
                "export FROM_BASH_PROFILE=yes\n",
            )
            .unwrap();
            fs::write(dir.path().join(".bashrc"), "export FROM_BASHRC=yes\n").unwrap();
            let process = os_map(&[("HOME", dir.path().to_str().unwrap())]);
            let script = OsStr::new(
                "printf '\\0clipboard_transformer_env_start_test\\0'; /usr/bin/env -0; printf '\\0clipboard_transformer_env_end_test\\0'",
            );

            let captured = capture_shell_environment(
                Path::new("/bin/bash"),
                &process,
                Duration::from_secs(2),
                MAX_CAPTURE_BYTES,
                script,
                b"clipboard_transformer_env_start_test",
                b"clipboard_transformer_env_end_test",
            )
            .unwrap();

            assert_eq!(
                captured.get(OsStr::new("FROM_BASH_PROFILE")),
                Some(&"yes".into())
            );
            assert!(!captured.contains_key(OsStr::new("FROM_BASHRC")));
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn zsh_capture_is_login_but_not_interactive() {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join(".zshenv"), "export FROM_ZSHENV=yes\n").unwrap();
            fs::write(dir.path().join(".zprofile"), "export FROM_ZPROFILE=yes\n").unwrap();
            fs::write(dir.path().join(".zshrc"), "export FROM_ZSHRC=yes\n").unwrap();
            fs::write(dir.path().join(".zlogin"), "export FROM_ZLOGIN=yes\n").unwrap();
            let process = os_map(&[("HOME", "/tmp"), ("ZDOTDIR", dir.path().to_str().unwrap())]);

            let script = OsStr::new(
                "printf '\\0clipboard_transformer_env_start_test\\0'; /usr/bin/env -0; printf '\\0clipboard_transformer_env_end_test\\0'",
            );
            let captured = capture_shell_environment(
                Path::new("/bin/zsh"),
                &process,
                Duration::from_secs(2),
                MAX_CAPTURE_BYTES,
                script,
                b"clipboard_transformer_env_start_test",
                b"clipboard_transformer_env_end_test",
            )
            .unwrap();

            assert_eq!(captured.get(OsStr::new("FROM_ZSHENV")), Some(&"yes".into()));
            assert_eq!(
                captured.get(OsStr::new("FROM_ZPROFILE")),
                Some(&"yes".into())
            );
            assert_eq!(captured.get(OsStr::new("FROM_ZLOGIN")), Some(&"yes".into()));
            assert!(!captured.contains_key(OsStr::new("FROM_ZSHRC")));
        }
    }
}

#[cfg(unix)]
pub use unix::BootstrapReport;

#[cfg(not(unix))]
#[derive(Debug, Clone)]
pub struct BootstrapReport {
    pub imported_count: usize,
    pub warning: Option<String>,
}

#[cfg(unix)]
pub fn bootstrap_gui_environment() -> BootstrapReport {
    let report = unix::bootstrap_gui_environment();
    ENVIRONMENT_REVISION.fetch_add(1, Ordering::Relaxed);
    report
}

#[cfg(not(unix))]
pub fn bootstrap_gui_environment() -> BootstrapReport {
    BootstrapReport {
        imported_count: 0,
        warning: None,
    }
}

#[cfg(unix)]
fn refresh_gui_environment() -> anyhow::Result<bool> {
    let changed = unix::refresh_gui_environment()?;
    if changed {
        ENVIRONMENT_REVISION.fetch_add(1, Ordering::Relaxed);
    }
    Ok(changed)
}

#[cfg(not(unix))]
fn refresh_gui_environment() -> anyhow::Result<bool> {
    Ok(false)
}

#[cfg(unix)]
pub fn dump_current_environment(marker: &str) -> std::io::Result<()> {
    unix::dump_current_environment(marker)
}

pub fn refresh_for_config(
    config_path: &Path,
    mode: EnvironmentRefreshMode,
) -> EnvironmentRefreshReport {
    let shell_warning = match mode {
        EnvironmentRefreshMode::Background => None,
        EnvironmentRefreshMode::Explicit => refresh_gui_environment()
            .err()
            .map(|error| format!("{error:#}")),
    };
    let (dotenv, dotenv_changed) = load_dotenv_for_config(config_path);
    EnvironmentRefreshReport {
        revision: revision(),
        dotenv,
        dotenv_changed,
        shell_warning,
    }
}

pub fn load_dotenv_for_config(config_path: &Path) -> (DotenvReport, bool) {
    let path = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".env");
    let present = path.exists();
    let read = read_dotenv(&path);
    let next = DotenvSnapshot {
        path: path.clone(),
        values: read.values,
    };
    let changed = if let Some(snapshot) = DOTENV.get() {
        let mut snapshot = snapshot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let changed = *snapshot != next;
        *snapshot = next;
        changed
    } else {
        let changed = !next.values.is_empty();
        let _ = DOTENV.set(RwLock::new(next));
        changed
    };
    if changed {
        ENVIRONMENT_REVISION.fetch_add(1, Ordering::Relaxed);
    }
    (
        DotenvReport {
            path,
            present,
            loaded_count: dotenv_values().len(),
            ignored_count: read.ignored_count,
        },
        changed,
    )
}

pub fn dotenv_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".env")
}

pub fn revision() -> u64 {
    ENVIRONMENT_REVISION.load(Ordering::Relaxed)
}

/// Reports whether the desktop process is running inside a Flatpak sandbox.
///
/// Environment detection belongs to the runtime host. Portable native
/// adapters receive the resulting capability through their options instead of
/// sampling process state independently.
pub fn running_in_flatpak() -> bool {
    cfg!(target_os = "linux") && identifies_flatpak(std::env::var_os("FLATPAK_ID").as_deref())
}

fn identifies_flatpak(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

fn read_dotenv(path: &Path) -> DotenvRead {
    if !path.exists() {
        return DotenvRead::default();
    }
    let Ok(entries) = dotenvy::from_path_iter(path) else {
        return DotenvRead {
            ignored_count: 1,
            ..DotenvRead::default()
        };
    };
    let mut read = DotenvRead::default();
    for entry in entries {
        match entry {
            Ok((name, value)) => {
                read.values
                    .entry(OsString::from(name))
                    .or_insert_with(|| OsString::from(value));
            }
            Err(error) => {
                read.ignored_count += 1;
                // dotenvy's iterator advances after syntax errors, allowing
                // later valid entries to load. A read error may be returned
                // repeatedly without consuming input, so it ends this read.
                if matches!(error, dotenvy::Error::Io(_)) {
                    break;
                }
            }
        }
    }
    read
}

fn dotenv_values() -> BTreeMap<OsString, OsString> {
    DOTENV
        .get()
        .and_then(|snapshot| snapshot.read().ok())
        .map(|snapshot| snapshot.values.clone())
        .unwrap_or_default()
}

pub fn var(name: &str) -> Option<String> {
    let dotenv = dotenv_values();
    #[cfg(unix)]
    if unix::is_active() {
        return unix::var_with_dotenv(name, &dotenv);
    }
    std::env::var(name).ok().or_else(|| {
        dotenv
            .get(OsStr::new(name))
            .and_then(|value| value.to_str())
            .map(str::to_owned)
    })
}

pub fn configure_command(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

        command.creation_flags(CREATE_NO_WINDOW);
    }

    let dotenv = dotenv_values();
    #[cfg(unix)]
    if let Some(environment) = unix::command_environment(&dotenv) {
        command.env_clear().envs(environment);
        return;
    }
    if DOTENV.get().is_some() {
        let mut environment = dotenv;
        environment.extend(std::env::vars_os());
        command.env_clear().envs(environment);
    }
}

#[cfg(test)]
mod dotenv_tests {
    use std::fs;

    use super::*;

    #[test]
    fn reads_dotenv_without_overwriting_the_first_declaration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        fs::write(&path, "TOKEN=first\nTOKEN=second\nQUOTED=\"hello world\"\n").unwrap();

        let read = read_dotenv(&path);

        assert_eq!(read.values.get(OsStr::new("TOKEN")), Some(&"first".into()));
        assert_eq!(
            read.values.get(OsStr::new("QUOTED")),
            Some(&"hello world".into())
        );
        assert_eq!(read.ignored_count, 0);
    }

    #[test]
    fn reads_valid_dotenv_entries_around_invalid_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        fs::write(&path, "BEFORE=one\nthis is invalid\nAFTER=two\n").unwrap();

        let read = read_dotenv(&path);

        assert_eq!(read.values.get(OsStr::new("BEFORE")), Some(&"one".into()));
        assert_eq!(read.values.get(OsStr::new("AFTER")), Some(&"two".into()));
        assert_eq!(read.ignored_count, 1);
    }

    #[test]
    fn unreadable_dotenv_is_an_empty_layer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        fs::create_dir(&path).unwrap();

        let read = read_dotenv(&path);

        assert!(read.values.is_empty());
        assert_eq!(read.ignored_count, 1);
    }

    #[test]
    fn unreadable_reload_replaces_the_previous_dotenv_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.yaml");
        let path = dir.path().join(".env");
        let name = "CLIPBOARD_TRANSFORMER_DOTENV_REPLACEMENT_TEST";
        fs::write(&path, format!("{name}=old\n")).unwrap();

        let (loaded, _) = load_dotenv_for_config(&config);
        assert_eq!(loaded.loaded_count, 1);
        assert_eq!(dotenv_values().get(OsStr::new(name)), Some(&"old".into()));

        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let (ignored, changed) = load_dotenv_for_config(&config);

        assert!(changed);
        assert_eq!(ignored.loaded_count, 0);
        assert_eq!(ignored.ignored_count, 1);
        assert!(!dotenv_values().contains_key(OsStr::new(name)));
    }
}

#[cfg(test)]
mod platform_tests {
    use super::*;

    #[test]
    fn flatpak_requires_a_non_empty_application_id() {
        assert!(!identifies_flatpak(None));
        assert!(!identifies_flatpak(Some(OsStr::new(""))));
        assert!(identifies_flatpak(Some(OsStr::new(
            "dev.jag_k.clipboard_transformer"
        ))));
    }
}
