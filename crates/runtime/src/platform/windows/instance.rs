use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use anyhow::{bail, Context, Result};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;

const INSTANCE_MUTEX_NAME: &str = "Local\\dev.jag-k.clipboard-transformer";

pub struct InstanceGuard {
    mutex: HANDLE,
}

impl InstanceGuard {
    /// Claims the per-user desktop instance after configuration has loaded.
    ///
    /// Notification activations received by the running process do not create a
    /// second instance. A separate executable launch exits instead of creating
    /// another tray and clipboard watcher.
    pub fn claim() -> Result<Self> {
        let name = wide(INSTANCE_MUTEX_NAME);
        let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if mutex.is_null() {
            return Err(std::io::Error::last_os_error()).context("create Windows instance mutex");
        }

        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe { CloseHandle(mutex) };
            bail!("Clipboard Transformer is already running")
        }

        Ok(Self { mutex })
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.mutex) };
    }
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutex_name_is_local_and_stable() {
        assert!(INSTANCE_MUTEX_NAME.starts_with("Local\\"));
        assert!(INSTANCE_MUTEX_NAME.contains("dev.jag-k.clipboard-transformer"));
        assert_eq!(wide(INSTANCE_MUTEX_NAME).last(), Some(&0));
    }
}
