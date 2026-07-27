use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
#[cfg(not(target_os = "linux"))]
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::logging;

const SHUTDOWN_WAIT_STEPS: usize = 20;
const SHUTDOWN_WAIT_STEP: Duration = Duration::from_millis(100);

/// How long to keep retrying a non-blocking `flock` before reporting the lock as
/// held by someone else.
///
/// macOS does not make the release performed by `close()` visible to the next
/// non-blocking attempt immediately: under load, measured on this repository's
/// own test sequence, roughly 5% of attempts returned `EWOULDBLOCK` and every
/// one of them succeeded on a retry between 100µs and 10ms later. A single
/// attempt therefore cannot tell "another instance owns this" from "the previous
/// owner's descriptor is not released yet", and the difference decides whether
/// the app starts at all. The budget is an order of magnitude above the worst
/// delay observed, and it is only ever spent when the lock really is contended.
const LOCK_RETRY_BUDGET: Duration = Duration::from_millis(250);
const LOCK_RETRY_STEP: Duration = Duration::from_millis(2);

#[derive(Debug)]
pub struct InstanceGuard {
    _lock_file: File,
}

impl InstanceGuard {
    pub fn restart_previous(pid_file: PathBuf) -> Result<Self> {
        let pid = std::process::id();
        if let Some(parent) = pid_file.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }

        let mut lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&pid_file)
            .with_context(|| format!("open {}", pid_file.display()))?;

        // Both attempts retry, because both can land in the window where a
        // just-departed owner's release is not visible yet: the first when the
        // user relaunches immediately after quitting, the second right after
        // `stop_previous` reaped the old process. Concluding from one attempt
        // turns that window into a refusal to start.
        if !lock_exclusive_briefly(&lock_file)? {
            let Some(previous_pid) = read_pid(&mut lock_file)? else {
                bail!(
                    "another Clipboard Transformer instance owns {} without a valid PID",
                    pid_file.display()
                );
            };
            if previous_pid == pid || !process_is_clipboard_transformer(previous_pid) {
                bail!(
                    "another process owns the Clipboard Transformer instance lock at {}",
                    pid_file.display()
                );
            }

            stop_previous(previous_pid)?;
            if !lock_exclusive_briefly(&lock_file)? {
                bail!(
                    "another Clipboard Transformer instance still owns {}",
                    pid_file.display()
                );
            }
        }

        // Upgrade safely from versions that wrote a PID without holding a lock.
        if let Some(previous_pid) = read_pid(&mut lock_file)? {
            if previous_pid != pid && process_is_clipboard_transformer(previous_pid) {
                stop_previous(previous_pid)?;
            }
        }

        write_pid(&mut lock_file, pid).with_context(|| format!("write {}", pid_file.display()))?;
        logging::event(format!("instance guard active pid={pid}"));
        Ok(Self {
            _lock_file: lock_file,
        })
    }
}

/// Claims the exclusive lock, retrying within [`LOCK_RETRY_BUDGET`].
///
/// `false` means the lock stayed held for the whole budget, which is the only
/// evidence that another process really owns it. Errors other than "would block"
/// are still reported immediately: they say the call itself failed, not that
/// somebody else holds the file.
fn lock_exclusive_briefly(file: &File) -> Result<bool> {
    let deadline = Instant::now() + LOCK_RETRY_BUDGET;
    loop {
        if try_lock_exclusive(file)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(LOCK_RETRY_STEP);
    }
}

fn try_lock_exclusive(file: &File) -> Result<bool> {
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }

    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return Ok(false);
    }
    Err(error).context("lock Clipboard Transformer instance file")
}

fn read_pid(file: &mut File) -> Result<Option<u32>> {
    file.seek(SeekFrom::Start(0))
        .context("seek instance file")?;
    let mut content = String::new();
    file.read_to_string(&mut content)
        .context("read instance file")?;
    Ok(content
        .lines()
        .next()
        .and_then(|line| line.trim().parse().ok()))
}

fn write_pid(file: &mut File, pid: u32) -> Result<()> {
    file.set_len(0).context("truncate instance file")?;
    file.seek(SeekFrom::Start(0))
        .context("seek instance file")?;
    writeln!(file, "{pid}").context("write instance PID")?;
    file.flush().context("flush instance PID")
}

fn process_is_clipboard_transformer(pid: u32) -> bool {
    if !process_is_alive(pid) {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .is_some_and(|command| command.contains("clipboard-transformer"))
    }
    #[cfg(not(target_os = "linux"))]
    Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .is_some_and(|command| {
            command.contains("clipboard-transformer") || command.contains("Clipboard Transformer")
        })
}

fn process_is_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Ok(pid) = i32::try_from(pid) else {
            return false;
        };
        let result = unsafe { libc::kill(pid, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(target_os = "linux"))]
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
}

fn terminate_process(pid: u32) {
    #[cfg(target_os = "linux")]
    if let Ok(pid) = i32::try_from(pid) {
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = Command::new("/bin/kill")
        .args(["-TERM", &pid.to_string()])
        .status();
}

fn force_terminate_process(pid: u32) {
    #[cfg(target_os = "linux")]
    if let Ok(pid) = i32::try_from(pid) {
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = Command::new("/bin/kill")
        .args(["-KILL", &pid.to_string()])
        .status();
}

fn wait_until_stopped(pid: u32) -> bool {
    for _ in 0..SHUTDOWN_WAIT_STEPS {
        if !process_is_alive(pid) {
            return true;
        }
        thread::sleep(SHUTDOWN_WAIT_STEP);
    }
    !process_is_alive(pid)
}

fn stop_previous(pid: u32) -> Result<()> {
    logging::event(format!("terminating previous instance pid={pid}"));
    terminate_process(pid);
    if wait_until_stopped(pid) {
        return Ok(());
    }

    if process_is_clipboard_transformer(pid) {
        logging::event(format!("force terminating previous instance pid={pid}"));
        force_terminate_process(pid);
    }
    if wait_until_stopped(pid) {
        Ok(())
    } else {
        bail!("previous Clipboard Transformer instance pid={pid} did not stop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_pid_uses_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clipboard-transformer.pid");
        fs::write(&path, "123\nignored\n").unwrap();
        let mut file = OpenOptions::new().read(true).open(path).unwrap();

        assert_eq!(read_pid(&mut file).unwrap(), Some(123));
    }

    #[test]
    fn exclusive_lock_is_released_when_file_closes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clipboard-transformer.pid");
        let first = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        let second = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();

        assert!(try_lock_exclusive(&first).unwrap());
        assert!(!try_lock_exclusive(&second).unwrap());
        drop(first);
        // Through the retrying path, because that is what the app uses and what
        // the platform requires: macOS makes the release visible to the next
        // non-blocking attempt up to ~10ms later under load, so a single attempt
        // here was failing roughly half of the full parallel test runs.
        assert!(
            lock_exclusive_briefly(&second).unwrap(),
            "the lock must become available once no descriptor refers to it"
        );
    }

    #[test]
    fn guard_keeps_lock_file_after_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clipboard-transformer.pid");
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        {
            let guard = InstanceGuard {
                _lock_file: lock_file,
            };
            drop(guard);
        }

        assert!(path.exists());
    }

    /// The retry must not turn a genuinely held lock into a claimable one: that
    /// is what keeps single-instance behavior meaningful. It also has to give up
    /// rather than wait forever, so the whole budget is spent and no more.
    #[test]
    fn a_lock_held_for_the_whole_budget_is_reported_as_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clipboard-transformer.pid");
        let held = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(try_lock_exclusive(&held).unwrap());

        let started = Instant::now();
        assert!(
            !lock_exclusive_briefly(&contender).unwrap(),
            "a held lock must stay reported as held"
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed >= LOCK_RETRY_BUDGET,
            "gave up after {elapsed:?}, before the {LOCK_RETRY_BUDGET:?} budget"
        );
        assert!(
            elapsed < LOCK_RETRY_BUDGET * 4,
            "waited {elapsed:?}, far beyond the {LOCK_RETRY_BUDGET:?} budget"
        );
    }
}
