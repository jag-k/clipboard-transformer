//! Owns the main thread and pumps native OS events.
//!
//! A desktop process must drain the OS event queue or the system considers it
//! hung, and each platform imposes where that must happen: macOS requires the
//! main thread for anything UI, Windows requires the thread that created the
//! window, and Linux needs the process to yield between iterations.
//!
//! The loop therefore keeps only the OS half of the work and *asks* for the
//! application half through a closure. Nothing here names an application type,
//! and nothing outside `apps/*` depends on this crate: the hook is a plain
//! `FnMut`, so neither the runtime nor the tray implements a trait defined here.

use anyhow::Result;

/// Whether the loop should keep running.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Control {
    Continue,
    Quit,
}

/// How long a single iteration may wait for an OS event before letting the
/// application tick again.
const POLL_INTERVAL_MS: u64 = 200;

/// Runs until `tick` reports [`Control::Quit`].
///
/// `tick` is the whole application half of one iteration: advance state, drain
/// queued commands, refresh visible chrome. It must not block, because the same
/// thread has to get back to the OS queue.
///
/// One closure rather than separate tick and refresh hooks: both would need to
/// capture the same application state, one mutably, so the borrow checker would
/// reject the pair. Ordering inside an iteration is the host's business anyway.
///
/// Must be called on the process main thread; macOS enforces that and returns an
/// error otherwise.
pub fn run<T>(mut tick: T) -> Result<()>
where
    T: FnMut() -> Result<Control>,
{
    let mut pump = platform::Pump::new()?;
    log::info!("native host loop started");
    loop {
        let control = tick()?;
        pump.wait_and_dispatch(POLL_INTERVAL_MS)?;
        if control == Control::Quit {
            break;
        }
    }
    log::info!("native host loop stopped");
    Ok(())
}

#[cfg(target_os = "macos")]
mod platform {
    use anyhow::{Context, Result};
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventMask};
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode};

    pub struct Pump {
        application: Retained<NSApplication>,
    }

    impl Pump {
        pub fn new() -> Result<Self> {
            let mtm =
                MainThreadMarker::new().context("native macOS host requires the main thread")?;
            let application = NSApplication::sharedApplication(mtm);
            application.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            application.finishLaunching();
            Ok(Self { application })
        }

        pub fn wait_and_dispatch(&mut self, timeout_ms: u64) -> Result<()> {
            // `NSApplication::run` would install autorelease pools around event
            // dispatch itself. This loop owns the pump, so it must provide the
            // same boundary or temporary AppKit objects accumulate for the
            // lifetime of the process.
            autoreleasepool(|_| {
                let until = NSDate::dateWithTimeIntervalSinceNow(timeout_ms as f64 / 1000.0);
                if let Some(event) = self
                    .application
                    .nextEventMatchingMask_untilDate_inMode_dequeue(
                        NSEventMask::Any,
                        Some(&until),
                        unsafe { NSDefaultRunLoopMode },
                        true,
                    )
                {
                    self.application.sendEvent(&event);
                }
                self.application.updateWindows();
            });
            Ok(())
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::ptr;

    use anyhow::Result;
    use windows_sys::Win32::Foundation::WAIT_FAILED;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MsgWaitForMultipleObjectsEx, PeekMessageW, TranslateMessage, MSG,
        MWMO_INPUTAVAILABLE, PM_REMOVE, QS_ALLINPUT, WM_QUIT,
    };

    pub struct Pump;

    impl Pump {
        pub fn new() -> Result<Self> {
            Ok(Self)
        }

        pub fn wait_and_dispatch(&mut self, timeout_ms: u64) -> Result<()> {
            unsafe {
                let wait = MsgWaitForMultipleObjectsEx(
                    0,
                    ptr::null(),
                    timeout_ms as u32,
                    QS_ALLINPUT,
                    MWMO_INPUTAVAILABLE,
                );
                if wait == WAIT_FAILED {
                    anyhow::bail!(
                        "wait for native Windows host events: {}",
                        std::io::Error::last_os_error()
                    );
                }
                let mut message = MSG::default();
                while PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    if message.message == WM_QUIT {
                        return Ok(());
                    }
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            Ok(())
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use anyhow::Result;

    /// Linux has no process-wide pump to drain: the StatusNotifierItem service
    /// owns its own thread, so this only yields the CPU between ticks.
    pub struct Pump;

    impl Pump {
        pub fn new() -> Result<Self> {
            Ok(Self)
        }

        pub fn wait_and_dispatch(&mut self, timeout_ms: u64) -> Result<()> {
            std::thread::sleep(std::time::Duration::from_millis(timeout_ms));
            Ok(())
        }
    }
}
