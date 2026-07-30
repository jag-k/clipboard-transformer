//! The application's log sink, exposed through the `log` facade.
//!
//! Installing a [`log::Log`] implementation is what lets crates outside this
//! one — including native platform crates that must not depend on the
//! application — emit diagnostics with `log::info!` and have them land in the
//! same file. [`event`] is kept as the in-crate spelling so existing call sites
//! and the on-disk line format are unchanged.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{Level, LevelFilter, Log, Metadata, Record};

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static LOG_LOCK: Mutex<()> = Mutex::new(());
static MIRROR_STDERR: AtomicBool = AtomicBool::new(false);

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const RETAINED_LOG_FILES: usize = 3;
const FILE_LOG_LEVEL: LevelFilter = LevelFilter::Info;

/// Writes only the message, with no level or target prefix, so records routed
/// through `log` are byte-identical to the historical format.
struct FileLogger;

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            write_line(&record.args().to_string());
        }
    }

    fn flush(&self) {}
}

/// Idempotent: called from both entry points because the earliest diagnostics
/// happen before the log file path is known.
fn install() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        if log::set_logger(&FileLogger).is_ok() {
            log::set_max_level(FILE_LOG_LEVEL);
        }
    });
}

pub fn enable_stderr_mirror() {
    install();
    MIRROR_STDERR.store(true, Ordering::Relaxed);
}

pub fn init(path: impl AsRef<Path>) {
    install();
    let path = path.as_ref().to_path_buf();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = rotate_log_if_needed(&path, MAX_LOG_BYTES, RETAINED_LOG_FILES);
    let _ = LOG_PATH.set(path);
}

pub fn event(message: impl AsRef<str>) {
    write_line(message.as_ref());
}

fn write_line(message: &str) {
    let line = format!("{} {}\n", timestamp(), message);
    let _guard = LOG_LOCK.lock().ok();
    if let Some(path) = LOG_PATH.get() {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(line.as_bytes());
        }
    }
    // Console launches get an immediate diagnostic stream in addition to the
    // persistent log. GUI-subsystem Windows launches have no stderr handle;
    // that write simply fails and is intentionally ignored.
    if MIRROR_STDERR.load(Ordering::Relaxed) {
        let _ = std::io::stderr().write_all(line.as_bytes());
    }
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn rotate_log_if_needed(path: &Path, max_bytes: u64, retained_files: usize) -> io::Result<()> {
    let size = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if size <= max_bytes {
        return Ok(());
    }

    if retained_files == 0 {
        return fs::remove_file(path);
    }

    for index in (1..=retained_files).rev() {
        let destination = rotated_log_path(path, index);
        if destination.exists() {
            fs::remove_file(&destination)?;
        }
        let source = if index == 1 {
            path.to_path_buf()
        } else {
            rotated_log_path(path, index - 1)
        };
        if source.exists() {
            fs::rename(source, destination)?;
        }
    }
    Ok(())
}

fn rotated_log_path(path: &Path, index: usize) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{index}"));
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::{init, rotate_log_if_needed, rotated_log_path, FileLogger};
    use log::{Level, Log, Metadata};
    use std::fs;

    /// The whole point of installing a `log::Log` implementation: a crate that
    /// knows nothing about this one can emit `log::info!` and have it land in
    /// the application log, in the same format as `event`.
    ///
    /// One test owns this because the log path is process-global.
    #[test]
    fn records_routed_through_the_log_facade_reach_the_application_log() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("app.log");
        init(&path);

        super::event("written by event");
        log::info!("written by the {} facade", "log");
        log::warn!("warnings arrive too");

        let contents = fs::read_to_string(&path).unwrap();
        for expected in [
            "written by event",
            "written by the log facade",
            "warnings arrive too",
        ] {
            assert!(
                contents.contains(expected),
                "missing {expected:?} in {contents:?}"
            );
        }
        // Every line is `<unix seconds> <message>` with no level or target.
        for line in contents.lines() {
            let (timestamp, message) = line.split_once(' ').expect("timestamp and message");
            assert!(
                timestamp.parse::<u64>().is_ok(),
                "bad timestamp in {line:?}"
            );
            assert!(!message.starts_with("INFO"), "level leaked into {line:?}");
        }
    }

    #[test]
    fn dependency_debug_and_trace_records_are_disabled() {
        let logger = FileLogger;
        for level in [Level::Error, Level::Warn, Level::Info] {
            let metadata = Metadata::builder()
                .level(level)
                .target("dependency")
                .build();
            assert!(logger.enabled(&metadata));
        }
        for level in [Level::Debug, Level::Trace] {
            let metadata = Metadata::builder()
                .level(level)
                .target("dependency")
                .build();
            assert!(!logger.enabled(&metadata));
        }
    }

    #[test]
    fn log_below_limit_is_not_rotated() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("app.log");
        fs::write(&path, b"1234").unwrap();

        rotate_log_if_needed(&path, 4, 3).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"1234");
        assert!(!rotated_log_path(&path, 1).exists());
    }

    #[test]
    fn oversized_log_rotates_and_discards_oldest_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("app.log");
        fs::write(&path, b"current").unwrap();
        fs::write(rotated_log_path(&path, 1), b"previous-1").unwrap();
        fs::write(rotated_log_path(&path, 2), b"previous-2").unwrap();
        fs::write(rotated_log_path(&path, 3), b"discarded").unwrap();

        rotate_log_if_needed(&path, 3, 3).unwrap();

        assert!(!path.exists());
        assert_eq!(fs::read(rotated_log_path(&path, 1)).unwrap(), b"current");
        assert_eq!(fs::read(rotated_log_path(&path, 2)).unwrap(), b"previous-1");
        assert_eq!(fs::read(rotated_log_path(&path, 3)).unwrap(), b"previous-2");
        assert!(!rotated_log_path(&path, 4).exists());
    }

    #[test]
    fn oversized_log_is_removed_when_retention_is_disabled() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("app.log");
        fs::write(&path, b"oversized").unwrap();

        rotate_log_if_needed(&path, 1, 0).unwrap();

        assert!(!path.exists());
    }
}
