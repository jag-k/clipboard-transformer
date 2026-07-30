//! Owns the main thread and pumps native OS events.
//!
//! A desktop process must drain the OS event queue or the system considers it
//! hung, and each platform imposes where that must happen: macOS requires the
//! main thread for anything UI, Windows requires the thread that created the
//! window, and Linux needs the process to yield between iterations.
//!
//! The loop therefore owns native waiting, event dispatch, lost-wakeup
//! protection, application-defined reason coalescing, and platform event
//! reporting. It *asks* for the application half through a closure. Nothing
//! here names an application command or runtime type: the desktop host maps a
//! [`LoopInput`] into its own work model.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;

/// Whether the loop should keep running.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Control {
    Continue,
    Quit,
}

/// One application-defined reason for waking the host.
///
/// The host loop only coalesces bits; the desktop host assigns their meaning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WakeReason(u64);

impl WakeReason {
    pub const fn from_bit(bit: u8) -> Self {
        assert!(bit < 64, "wake reason bit must be below 64");
        Self(1_u64 << bit)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WakeReasons(u64);

impl WakeReasons {
    pub const fn contains(self, reason: WakeReason) -> bool {
        self.0 & reason.0 != 0
    }

    pub const fn with(self, reason: WakeReason) -> Self {
        Self(self.0 | reason.0)
    }
}

impl From<WakeReason> for WakeReasons {
    fn from(reason: WakeReason) -> Self {
        Self(reason.0)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeEvents {
    pub clipboard_changed: bool,
    pub quit_requested: bool,
}

impl NativeEvents {
    fn merge(&mut self, other: Self) {
        self.clipboard_changed |= other.clipboard_changed;
        self.quit_requested |= other.quit_requested;
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeCapabilities {
    pub clipboard_change_notifications: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LoopInput {
    pub reasons: WakeReasons,
    pub native_events: NativeEvents,
    pub native_capabilities: NativeCapabilities,
}

#[derive(Clone)]
pub struct WakeHandle {
    shared: Arc<WakeState>,
}

struct WakeState {
    pending: AtomicBool,
    reasons: std::sync::atomic::AtomicU64,
    native: platform::Waker,
}

impl WakeHandle {
    /// Creates the platform wake endpoint.
    ///
    /// Construct this on the same native-affine thread that will call
    /// [`run`]. Clones are thread-safe and may be sent to producers.
    pub fn new() -> Result<Self> {
        Ok(Self {
            shared: Arc::new(WakeState {
                pending: AtomicBool::new(false),
                reasons: std::sync::atomic::AtomicU64::new(0),
                native: platform::Waker::new()?,
            }),
        })
    }

    /// Makes the native wait return. Multiple outstanding wakeups coalesce,
    /// while the ready flag closes the final-drain-to-sleep race.
    pub fn wake(&self) {
        if !self.shared.pending.swap(true, Ordering::AcqRel) {
            self.shared.native.signal();
        }
    }

    pub fn wake_for(&self, reason: WakeReason) {
        self.shared.reasons.fetch_or(reason.0, Ordering::AcqRel);
        self.wake();
    }

    pub fn callback(&self, reason: WakeReason) -> Arc<dyn Fn() + Send + Sync> {
        let wake = self.clone();
        Arc::new(move || wake.wake_for(reason))
    }

    fn take_pending(&self) -> bool {
        self.shared.pending.swap(false, Ordering::AcqRel)
    }

    fn take_reasons(&self) -> WakeReasons {
        WakeReasons(
            self.shared
                .reasons
                .swap(0, std::sync::atomic::Ordering::AcqRel),
        )
    }

    fn native_capabilities(&self) -> NativeCapabilities {
        self.shared.native.capabilities()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Iteration {
    pub control: Control,
    pub deadline: Option<Instant>,
}

impl Iteration {
    pub const fn continue_until(deadline: Option<Instant>) -> Self {
        Self {
            control: Control::Continue,
            deadline,
        }
    }

    pub const fn quit() -> Self {
        Self {
            control: Control::Quit,
            deadline: None,
        }
    }
}

/// Runs until `tick` reports [`Control::Quit`].
///
/// `tick` receives coalesced application reasons and neutral native events.
/// It is the whole application half of one iteration: advance state, drain
/// queued commands, refresh visible chrome, and return the next real deadline.
/// It must not block, because the same thread has to get back to the OS queue.
/// The native pump is fully initialized before the first call, so the host may
/// create UI objects that require the platform application lifecycle there.
///
/// One closure rather than separate tick and refresh hooks: both would need to
/// capture the same application state, one mutably, so the borrow checker would
/// reject the pair. Ordering inside an iteration is the host's business anyway.
///
/// Must be called on the process main thread; macOS enforces that and returns an
/// error otherwise.
pub fn run<T>(wake: &WakeHandle, tick: T) -> Result<()>
where
    T: FnMut(LoopInput) -> Result<Iteration>,
{
    wake.shared.native.validate_run_thread()?;
    let mut pump = platform::Pump::new()?;
    log::info!("native host loop started");
    drive(&mut pump, wake, tick)?;
    log::info!("native host loop stopped");
    Ok(())
}

trait EventPump {
    fn wait_and_dispatch(
        &mut self,
        timeout: Option<Duration>,
        waker: &platform::Waker,
    ) -> Result<NativeEvents>;
}

fn drive<P, T>(pump: &mut P, wake: &WakeHandle, mut tick: T) -> Result<()>
where
    P: EventPump,
    T: FnMut(LoopInput) -> Result<Iteration>,
{
    let native_capabilities = wake.native_capabilities();
    let mut native_events = NativeEvents::default();
    loop {
        wake.take_pending();
        let iteration = tick(LoopInput {
            reasons: wake.take_reasons(),
            native_events: std::mem::take(&mut native_events),
            native_capabilities,
        })?;
        if iteration.control == Control::Quit {
            break;
        }
        // A producer may enqueue work after the application performed its
        // final drain but before it returned the deadline. Observe the token
        // before entering the OS wait so that wake cannot be lost.
        if wake.take_pending() {
            continue;
        }
        let timeout = iteration
            .deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()));
        native_events.merge(pump.wait_and_dispatch(timeout, &wake.shared.native)?);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::time::Duration;

    use anyhow::{Context, Result};
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventMask};
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode};

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRunLoopGetMain() -> *mut c_void;
        fn CFRunLoopWakeUp(run_loop: *mut c_void);
    }

    pub struct Waker {
        run_loop: NonNull<c_void>,
    }

    unsafe impl Send for Waker {}
    unsafe impl Sync for Waker {}

    impl Waker {
        pub fn new() -> Result<Self> {
            let run_loop =
                NonNull::new(unsafe { CFRunLoopGetMain() }).context("get macOS main run loop")?;
            Ok(Self { run_loop })
        }

        pub fn signal(&self) {
            unsafe { CFRunLoopWakeUp(self.run_loop.as_ptr()) };
        }

        pub fn capabilities(&self) -> super::NativeCapabilities {
            super::NativeCapabilities::default()
        }

        pub fn validate_run_thread(&self) -> Result<()> {
            Ok(())
        }
    }

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

        pub fn wait_and_dispatch(&mut self, timeout: Option<Duration>) -> Result<()> {
            // `NSApplication::run` would install autorelease pools around event
            // dispatch itself. This loop owns the pump, so it must provide the
            // same boundary or temporary AppKit objects accumulate for the
            // lifetime of the process.
            autoreleasepool(|_| {
                let until = match timeout {
                    Some(timeout) => NSDate::dateWithTimeIntervalSinceNow(timeout.as_secs_f64()),
                    None => NSDate::distantFuture(),
                };
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
            });
            Ok(())
        }
    }

    impl super::EventPump for Pump {
        fn wait_and_dispatch(
            &mut self,
            timeout: Option<Duration>,
            _waker: &Waker,
        ) -> Result<super::NativeEvents> {
            self.wait_and_dispatch(timeout)?;
            Ok(super::NativeEvents::default())
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::ptr;
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
    use std::time::Duration;

    use anyhow::{Context, Result};
    use windows_sys::Win32::Foundation::{
        GetLastError, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WAIT_FAILED, WPARAM,
    };
    use windows_sys::Win32::System::DataExchange::{
        AddClipboardFormatListener, RemoveClipboardFormatListener,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::{GetCurrentThreadId, INFINITE};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        MsgWaitForMultipleObjectsEx, PeekMessageW, PostThreadMessageW, RegisterClassW,
        TranslateMessage, CS_NOCLOSE, HWND_MESSAGE, MSG, MWMO_INPUTAVAILABLE, PM_NOREMOVE,
        PM_REMOVE, QS_ALLINPUT, WM_APP, WM_CLIPBOARDUPDATE, WM_QUIT, WNDCLASSW,
    };

    const WAKE_MESSAGE: u32 = WM_APP + 0x43;

    pub struct Waker {
        thread_id: AtomicU32,
        clipboard_window: AtomicUsize,
    }

    impl Waker {
        pub fn new() -> Result<Self> {
            // Creating a queue before publishing the thread ID makes
            // PostThreadMessage reliable even if the first producer is fast.
            let mut message = MSG::default();
            unsafe {
                PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_NOREMOVE);
            }
            let clipboard_window = match create_clipboard_listener_window() {
                Ok(window) => window,
                Err(error) => {
                    log::warn!(
                        "Windows clipboard notifications unavailable; using fallback polling: {error:#}"
                    );
                    ptr::null_mut()
                }
            };
            Ok(Self {
                thread_id: AtomicU32::new(unsafe { GetCurrentThreadId() }),
                clipboard_window: AtomicUsize::new(clipboard_window as usize),
            })
        }

        pub fn signal(&self) {
            let thread_id = self.thread_id.load(Ordering::Acquire);
            if unsafe { PostThreadMessageW(thread_id, WAKE_MESSAGE, 0, 0) } == 0 {
                log::warn!(
                    "post Windows host wake message failed: {}",
                    std::io::Error::last_os_error()
                );
            }
        }

        pub fn capabilities(&self) -> super::NativeCapabilities {
            super::NativeCapabilities {
                clipboard_change_notifications: self.clipboard_window.load(Ordering::Acquire) != 0,
            }
        }

        pub fn validate_run_thread(&self) -> Result<()> {
            let current = unsafe { GetCurrentThreadId() };
            let owner = self.thread_id.load(Ordering::Acquire);
            if current != owner {
                anyhow::bail!(
                    "Windows host loop must run on the thread that created its wake handle"
                );
            }
            Ok(())
        }
    }

    impl Drop for Waker {
        fn drop(&mut self) {
            let window = self.clipboard_window.swap(0, Ordering::AcqRel) as HWND;
            if !window.is_null() {
                unsafe {
                    RemoveClipboardFormatListener(window);
                    DestroyWindow(window);
                }
            }
        }
    }

    unsafe extern "system" fn clipboard_window_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    fn create_clipboard_listener_window() -> Result<HWND> {
        let class_name: Vec<u16> = "ClipboardTransformerHostWake\0".encode_utf16().collect();
        let instance = unsafe { GetModuleHandleW(ptr::null()) };
        if instance.is_null() {
            return Err(std::io::Error::last_os_error()).context("get Windows host module");
        }
        let class = WNDCLASSW {
            style: CS_NOCLOSE,
            lpfnWndProc: Some(clipboard_window_proc),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0
            && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS
        {
            return Err(std::io::Error::last_os_error())
                .context("register Windows clipboard listener class");
        }
        let window = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                class_name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                ptr::null_mut(),
                instance,
                ptr::null(),
            )
        };
        if window.is_null() {
            return Err(std::io::Error::last_os_error())
                .context("create Windows clipboard listener window");
        }
        if unsafe { AddClipboardFormatListener(window) } == 0 {
            unsafe {
                DestroyWindow(window);
            }
            return Err(std::io::Error::last_os_error())
                .context("register Windows clipboard listener");
        }
        Ok(window)
    }

    pub struct Pump;

    impl Pump {
        pub fn new() -> Result<Self> {
            Ok(Self)
        }

        pub fn wait_and_dispatch(
            &mut self,
            timeout: Option<Duration>,
            _waker: &Waker,
        ) -> Result<super::NativeEvents> {
            let mut events = super::NativeEvents::default();
            unsafe {
                let timeout_ms = timeout
                    .map(|timeout| timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32)
                    .unwrap_or(INFINITE);
                let wait = MsgWaitForMultipleObjectsEx(
                    0,
                    ptr::null(),
                    timeout_ms,
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
                        events.quit_requested = true;
                        continue;
                    }
                    if message.message == WAKE_MESSAGE {
                        continue;
                    }
                    if message.message == WM_CLIPBOARDUPDATE {
                        events.clipboard_changed = true;
                    }
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            Ok(events)
        }
    }

    impl super::EventPump for Pump {
        fn wait_and_dispatch(
            &mut self,
            timeout: Option<Duration>,
            waker: &Waker,
        ) -> Result<super::NativeEvents> {
            self.wait_and_dispatch(timeout, waker)
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    use anyhow::Result;

    pub struct Waker {
        signal: Mutex<bool>,
        ready: Condvar,
    }

    impl Waker {
        pub fn new() -> Result<Self> {
            Ok(Self {
                signal: Mutex::new(false),
                ready: Condvar::new(),
            })
        }

        pub fn signal(&self) {
            if let Ok(mut signaled) = self.signal.lock() {
                *signaled = true;
                self.ready.notify_one();
            }
        }

        pub fn capabilities(&self) -> super::NativeCapabilities {
            super::NativeCapabilities::default()
        }

        pub fn validate_run_thread(&self) -> Result<()> {
            Ok(())
        }
    }

    /// Linux has no process-wide pump to drain: the StatusNotifierItem service
    /// owns its own thread, so this only yields the CPU between ticks.
    pub struct Pump;

    impl Pump {
        pub fn new() -> Result<Self> {
            Ok(Self)
        }

        pub fn wait_and_dispatch(
            &mut self,
            timeout: Option<Duration>,
            waker: &Waker,
        ) -> Result<super::NativeEvents> {
            let mut signaled = waker
                .signal
                .lock()
                .map_err(|_| anyhow::anyhow!("Linux host wake lock poisoned"))?;
            if *signaled {
                *signaled = false;
                return Ok(super::NativeEvents::default());
            }
            match timeout {
                Some(timeout) => {
                    let (next, _) = waker
                        .ready
                        .wait_timeout_while(signaled, timeout, |current| !*current)
                        .map_err(|_| anyhow::anyhow!("Linux host wake lock poisoned"))?;
                    signaled = next;
                }
                None => {
                    signaled = waker
                        .ready
                        .wait_while(signaled, |current| !*current)
                        .map_err(|_| anyhow::anyhow!("Linux host wake lock poisoned"))?;
                }
            }
            *signaled = false;
            Ok(super::NativeEvents::default())
        }
    }

    impl super::EventPump for Pump {
        fn wait_and_dispatch(
            &mut self,
            timeout: Option<Duration>,
            waker: &Waker,
        ) -> Result<super::NativeEvents> {
            self.wait_and_dispatch(timeout, waker)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakePump {
        waits: Vec<Option<Duration>>,
        on_wait: Option<Box<dyn FnOnce()>>,
        events: NativeEvents,
    }

    impl EventPump for FakePump {
        fn wait_and_dispatch(
            &mut self,
            timeout: Option<Duration>,
            _waker: &platform::Waker,
        ) -> Result<NativeEvents> {
            self.waits.push(timeout);
            if let Some(on_wait) = self.on_wait.take() {
                on_wait();
            }
            Ok(std::mem::take(&mut self.events))
        }
    }

    #[test]
    fn wake_before_wait_skips_the_wait_and_coalesces() {
        let wake = WakeHandle::new().unwrap();
        let mut pump = FakePump::default();
        let mut ticks = 0;
        drive(&mut pump, &wake, |_| {
            ticks += 1;
            if ticks == 1 {
                wake.wake();
                wake.wake();
                Ok(Iteration::continue_until(None))
            } else {
                Ok(Iteration::quit())
            }
        })
        .unwrap();
        assert!(pump.waits.is_empty());
        assert_eq!(ticks, 2);
    }

    #[test]
    fn wake_during_wait_and_spurious_returns_are_safe() {
        let wake = WakeHandle::new().unwrap();
        let wake_during_wait = wake.clone();
        let mut pump = FakePump {
            waits: Vec::new(),
            on_wait: Some(Box::new(move || wake_during_wait.wake())),
            events: NativeEvents::default(),
        };
        let mut ticks = 0;
        drive(&mut pump, &wake, |_| {
            ticks += 1;
            Ok(if ticks == 1 {
                Iteration::continue_until(None)
            } else {
                Iteration::quit()
            })
        })
        .unwrap();
        assert_eq!(pump.waits, vec![None]);
    }

    #[test]
    fn deadline_is_forwarded_to_the_pump() {
        let wake = WakeHandle::new().unwrap();
        let mut pump = FakePump::default();
        let mut ticks = 0;
        drive(&mut pump, &wake, |_| {
            ticks += 1;
            Ok(if ticks == 1 {
                Iteration::continue_until(Some(Instant::now() + Duration::from_secs(1)))
            } else {
                Iteration::quit()
            })
        })
        .unwrap();
        let timeout = pump.waits[0].unwrap();
        assert!(timeout <= Duration::from_secs(1));
        assert!(timeout > Duration::from_millis(900));
    }

    #[test]
    fn application_reasons_coalesce_without_host_policy() {
        let first = WakeReason::from_bit(0);
        let second = WakeReason::from_bit(7);
        let wake = WakeHandle::new().unwrap();
        wake.wake_for(first);
        wake.wake_for(second);
        let mut pump = FakePump::default();
        let mut observed = WakeReasons::default();

        drive(&mut pump, &wake, |input| {
            observed = input.reasons;
            Ok(Iteration::quit())
        })
        .unwrap();

        assert!(observed.contains(first));
        assert!(observed.contains(second));
        assert!(pump.waits.is_empty());
    }

    #[test]
    fn native_events_are_delivered_on_the_next_drain() {
        let wake = WakeHandle::new().unwrap();
        let mut pump = FakePump {
            events: NativeEvents {
                clipboard_changed: true,
                quit_requested: false,
            },
            ..FakePump::default()
        };
        let mut ticks = 0;

        drive(&mut pump, &wake, |input| {
            ticks += 1;
            if ticks == 1 {
                assert_eq!(input.native_events, NativeEvents::default());
                Ok(Iteration::continue_until(Some(Instant::now())))
            } else {
                assert!(input.native_events.clipboard_changed);
                Ok(Iteration::quit())
            }
        })
        .unwrap();
    }
}
