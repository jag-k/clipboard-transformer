use anyhow::Result;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use super::{reload::ConfigReloader, Agent, AppCommand, AppEffect, AppEvent};
use crate::platform::autostart::AutostartStatus;
use crate::platform::tray::TraySnapshot;
use ct_clipboard::ClipboardBackend;
use ct_notifications::NotificationBackend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeControl {
    Continue,
    Quit,
}

const CLIPBOARD_CONTENTION_RETRY: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimeWork {
    pub commands: bool,
    pub rule_results: bool,
    pub config_events: bool,
    pub clipboard_changed: bool,
}

impl RuntimeWork {
    pub const fn all() -> Self {
        Self {
            commands: true,
            rule_results: true,
            config_events: true,
            clipboard_changed: false,
        }
    }
}

pub struct Runtime<'a, C, N> {
    agent: &'a mut Agent<C, N>,
    reloader: Option<&'a mut ConfigReloader>,
    commands: &'a Receiver<AppCommand>,
    last_clipboard_change_count: Option<u64>,
    clipboard_fallback_interval: Duration,
    clipboard_notifications: bool,
    next_clipboard_poll: Option<Instant>,
}

impl<'a, C, N> Runtime<'a, C, N>
where
    C: ClipboardBackend,
    N: NotificationBackend,
{
    pub fn new(
        agent: &'a mut Agent<C, N>,
        reloader: Option<&'a mut ConfigReloader>,
        commands: &'a Receiver<AppCommand>,
    ) -> Self {
        let clipboard_fallback_interval = agent.clipboard_fallback_interval();
        Self {
            agent,
            reloader,
            commands,
            last_clipboard_change_count: None,
            clipboard_fallback_interval,
            clipboard_notifications: false,
            next_clipboard_poll: Some(Instant::now()),
        }
    }

    /// Points the agent at the state the tray's menu source reads.
    ///
    /// After this, [`Self::process_pending`] republishes at the end of any tick
    /// that changed something, so no host loop needs to poll for one.
    pub fn attach_tray_state(&mut self, handle: crate::platform::tray::TrayStateHandle) {
        self.agent.attach_tray_state(handle);
    }

    /// The application half of one host-loop iteration.
    ///
    /// Deliberately returns [`RuntimeControl`] rather than the host loop's own
    /// `Control`: naming that type here would make every consumer of this crate
    /// — including the CLI, which owns no loop — depend on `ct-host-loop`. The
    /// two-variant conversion belongs to the host that runs the loop.
    pub fn process_pending(&mut self) -> Result<RuntimeControl> {
        let now = Instant::now();
        // Compatibility helper for non-event-loop callers and focused tests:
        // explicitly requesting a full poll includes the clipboard source.
        self.next_clipboard_poll = Some(now);
        self.process(RuntimeWork::all(), now)
    }

    /// Drains only the sources that woke the host, plus timers whose explicit
    /// deadlines have elapsed.
    pub fn process(&mut self, work: RuntimeWork, now: Instant) -> Result<RuntimeControl> {
        // Tracks whether this tick changed anything, so tray-visible state is
        // republished exactly when it can have changed and never on a timer.
        // Every branch below that touches the agent sets it; that is the whole
        // discipline, and it lives in this one function.
        let mut changed = if work.rule_results {
            self.agent.poll_rule_results()?
        } else {
            false
        };

        let reload_due = self
            .reloader
            .as_deref()
            .and_then(ConfigReloader::next_deadline)
            .is_some_and(|deadline| deadline <= now);
        let reload_outcome = match self.reloader.as_deref_mut() {
            Some(reloader) if work.config_events || reload_due => reloader.poll()?,
            Some(_) | None => None,
        };
        if let Some(outcome) = reload_outcome {
            changed = true;
            let effects = self.agent.handle_event(AppEvent::ConfigReloaded(outcome))?;
            if self.execute_effects(effects)? == RuntimeControl::Quit {
                return Ok(RuntimeControl::Quit);
            }
        }

        if work.commands {
            while let Ok(command) = self.commands.try_recv() {
                changed = true;
                match self.agent.handle_event(AppEvent::UserCommand(command)) {
                    Ok(effects) => {
                        if self.execute_effects(effects)? == RuntimeControl::Quit {
                            return Ok(RuntimeControl::Quit);
                        }
                    }
                    // A failed command (e.g. a clipboard write refused by another
                    // app) must not take the whole agent down.
                    Err(error) => crate::logging::event(format!("command failed: {error:#}")),
                }
            }
        }

        let clipboard_due = self
            .next_clipboard_poll
            .is_some_and(|deadline| now >= deadline);
        if work.clipboard_changed || clipboard_due {
            match self
                .agent
                .poll_clipboard(&mut self.last_clipboard_change_count)
            {
                Ok(Some(content)) => {
                    self.schedule_clipboard_fallback(now);
                    changed = true;
                    match self.agent.handle_event(AppEvent::ClipboardChanged(content)) {
                        Ok(effects) => {
                            if self.execute_effects(effects)? == RuntimeControl::Quit {
                                return Ok(RuntimeControl::Quit);
                            }
                        }
                        Err(error) => {
                            crate::logging::event(format!("clipboard transform failed: {error:#}"))
                        }
                    }
                }
                Ok(None) => {
                    self.schedule_clipboard_fallback(now);
                }
                // Transient clipboard contention gets a dedicated short retry
                // deadline instead of relying on an unrelated host tick.
                Err(error) => {
                    self.next_clipboard_poll = Some(now + CLIPBOARD_CONTENTION_RETRY);
                    crate::logging::event(format!("clipboard poll failed: {error:#}"));
                }
            }
        }

        if changed {
            self.agent.publish_tray_state();
        }
        Ok(RuntimeControl::Continue)
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        let reload = self
            .reloader
            .as_deref()
            .and_then(ConfigReloader::next_deadline);
        match (reload, self.next_clipboard_poll) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
            (None, None) => None,
        }
    }

    /// Uses reliable native clipboard notifications after the mandatory first
    /// observation. Transient read failures still install a retry deadline.
    pub fn set_clipboard_notifications(&mut self, enabled: bool) {
        self.clipboard_notifications = enabled;
    }

    fn schedule_clipboard_fallback(&mut self, now: Instant) {
        self.next_clipboard_poll =
            (!self.clipboard_notifications).then_some(now + self.clipboard_fallback_interval);
    }

    pub fn tray_snapshot(&self) -> TraySnapshot {
        self.agent.tray_snapshot()
    }

    #[cfg(test)]
    fn tray_publish_count(&self) -> u64 {
        self.agent.tray_publish_count()
    }

    fn execute_effects(&mut self, effects: Vec<AppEffect>) -> Result<RuntimeControl> {
        for effect in effects {
            match effect {
                AppEffect::OpenEditor { target, editor } => {
                    if let Err(error) = crate::platform::open::open_rule_in_editor(
                        std::path::Path::new(&target.path),
                        target.line,
                        editor.as_ref(),
                    ) {
                        crate::logging::event(format!("open config failed: {error:#}"));
                    }
                }
                AppEffect::ReloadConfig => {
                    let reload_outcome = match self.reloader.as_deref_mut() {
                        Some(reloader) => reloader.reload()?,
                        None => None,
                    };
                    if let Some(outcome) = reload_outcome {
                        let nested = self.agent.handle_event(AppEvent::ConfigReloaded(outcome))?;
                        if self.execute_effects(nested)? == RuntimeControl::Quit {
                            return Ok(RuntimeControl::Quit);
                        }
                    }
                }
                AppEffect::OpenConfig(path) => {
                    if let Err(error) = crate::platform::open::open_config(&path) {
                        crate::logging::event(format!("open config failed: {error:#}"));
                    }
                }
                AppEffect::RevealConfig(path) => {
                    if let Err(error) = crate::platform::open::reveal_config(&path) {
                        crate::logging::event(format!("reveal config failed: {error:#}"));
                    }
                }
                AppEffect::SetAutostart(enabled) => {
                    let result = if enabled {
                        crate::platform::autostart::enable_current()
                    } else {
                        crate::platform::autostart::disable()
                    };

                    match result {
                        Ok(()) => self.agent.set_autostart_status(if enabled {
                            AutostartStatus::Enabled
                        } else {
                            AutostartStatus::Disabled
                        }),
                        Err(error) => {
                            crate::logging::event(format!("set autostart failed: {error:#}"));
                            self.agent
                                .set_autostart_status(AutostartStatus::Error(error.to_string()));
                        }
                    }
                }
                AppEffect::Quit => return Ok(RuntimeControl::Quit),
            }
        }
        Ok(RuntimeControl::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, ConfigDocument};
    use ct_clipboard::ClipboardItem;
    use ct_clipboard::MemoryClipboardBackend;
    use ct_core::RawRule;
    use ct_notifications::NoopNotificationBackend;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn agent(initial: Option<&str>) -> Agent<MemoryClipboardBackend, NoopNotificationBackend> {
        Agent::new(
            MemoryClipboardBackend::new(initial.map(ClipboardItem::from_text)),
            NoopNotificationBackend::default(),
            ConfigDocument {
                plugins: Default::default(),
                config: AppConfig {
                    persist_last_clipboard: true,
                    ..AppConfig::default()
                },
                rules: vec![RawRule {
                    id: "rule".into(),
                    from: Some("cat".into()),
                    to: Some("dog".into()),
                    ..RawRule::default()
                }],
            },
        )
        .unwrap()
    }

    /// Publishing is driven by the tick, not by individual mutation sites. A
    /// transform therefore has to reach the tray's state without anyone asking,
    /// or the menu shows stale history until some unrelated change happens.
    #[test]
    fn a_tick_that_transforms_publishes_tray_state() {
        use crate::platform::tray::TrayStateHandle;

        let (_commands, receiver) = std::sync::mpsc::channel();
        let mut agent = agent(Some("cat"));
        let tray_state = TrayStateHandle::new(agent.tray_snapshot());
        agent.attach_tray_state(tray_state.clone());
        let menu = tray_state.source();
        let before = menu().items.len();
        let baseline = tray_state.generation();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut runtime = Runtime::new(&mut agent, None, &receiver);
        // Ticks until the worker's result lands and the tick that handled it
        // publishes. No test-only hook: this is the production path.
        while menu().items.len() == before {
            assert_eq!(runtime.process_pending().unwrap(), RuntimeControl::Continue);
            assert!(
                std::time::Instant::now() < deadline,
                "tray state was never published after a transform"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            tray_state.generation() > baseline,
            "the menu changed, so the state must have been written"
        );
    }

    /// An idle tick must not republish: doing so on a timer is what closes a menu
    /// the user has open on platforms where publishing is observable.
    #[test]
    fn an_idle_tick_publishes_nothing() {
        use crate::platform::tray::TrayStateHandle;

        let (_commands, receiver) = std::sync::mpsc::channel();
        let mut agent = agent(None);
        let handle = TrayStateHandle::new(agent.tray_snapshot());
        agent.attach_tray_state(handle.clone());
        // `attach` seeds once; from here nothing has happened, so nothing may be
        // written again.
        let mut runtime = Runtime::new(&mut agent, None, &receiver);
        let baseline = runtime.tray_publish_count();
        for _ in 0..5 {
            assert_eq!(runtime.process_pending().unwrap(), RuntimeControl::Continue);
        }

        // Counts rebuilds, not writes: an identical snapshot compares equal, so
        // `generation` alone would not notice a per-tick rebuild.
        assert_eq!(
            runtime.tray_publish_count(),
            baseline,
            "idle ticks must not rebuild tray state"
        );
        // Seeding stored a snapshot equal to the one the handle was built with, so
        // nothing was actually written either.
        assert_eq!(handle.generation(), 0, "idle ticks must not write state");
    }

    #[test]
    fn processes_clipboard_event_without_owning_a_loop() {
        let (_commands, receiver) = std::sync::mpsc::channel();
        let mut agent = agent(Some("cat"));
        {
            let mut runtime = Runtime::new(&mut agent, None, &receiver);
            assert_eq!(runtime.process_pending().unwrap(), RuntimeControl::Continue);
        }

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while agent.notifications.delivered.is_empty() {
            agent.poll_rule_results().unwrap();
            assert!(
                std::time::Instant::now() < deadline,
                "rule worker did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        assert_eq!(agent.notifications.delivered.len(), 1);
        assert_eq!(agent.clipboard.read().unwrap().unwrap().text(), Some("dog"));
    }

    #[test]
    fn quit_command_stops_before_processing_more_sources() {
        let (commands, receiver) = std::sync::mpsc::channel();
        commands.send(AppCommand::Quit).unwrap();
        let mut agent = agent(Some("cat"));
        {
            let mut runtime = Runtime::new(&mut agent, None, &receiver);
            assert_eq!(runtime.process_pending().unwrap(), RuntimeControl::Quit);
        }

        assert!(agent.notifications.delivered.is_empty());
        assert_eq!(agent.clipboard.read().unwrap().unwrap().text(), Some("cat"));
    }

    #[derive(Clone)]
    struct ObservedClipboard {
        state: Rc<RefCell<ObservedClipboardState>>,
    }

    struct ObservedClipboardState {
        item: Option<ClipboardItem>,
        change_count: u64,
        change_count_calls: usize,
        read_calls: usize,
    }

    impl ObservedClipboard {
        fn new(item: &str) -> (Self, Rc<RefCell<ObservedClipboardState>>) {
            let state = Rc::new(RefCell::new(ObservedClipboardState {
                item: Some(ClipboardItem::from_text(item)),
                change_count: 1,
                change_count_calls: 0,
                read_calls: 0,
            }));
            (
                Self {
                    state: state.clone(),
                },
                state,
            )
        }
    }

    impl ClipboardBackend for ObservedClipboard {
        fn change_count(&mut self) -> Result<Option<u64>> {
            let mut state = self.state.borrow_mut();
            state.change_count_calls += 1;
            Ok(Some(state.change_count))
        }

        fn read(&mut self) -> Result<Option<ClipboardItem>> {
            let mut state = self.state.borrow_mut();
            state.read_calls += 1;
            Ok(state.item.clone())
        }

        fn write(&mut self, item: &ClipboardItem) -> Result<()> {
            let mut state = self.state.borrow_mut();
            state.item = Some(item.clone());
            state.change_count += 1;
            Ok(())
        }
    }

    #[test]
    fn pause_keeps_observing_clipboard_without_transforming() {
        let (commands, receiver) = std::sync::mpsc::channel();
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state");
        let (clipboard, observed) = ObservedClipboard::new("bird");
        let mut agent = Agent::new(
            clipboard,
            NoopNotificationBackend::default(),
            ConfigDocument {
                plugins: Default::default(),
                config: AppConfig {
                    persist_last_clipboard: true,
                    ..AppConfig::default()
                },
                rules: vec![RawRule {
                    id: "rule".into(),
                    from: Some("cat".into()),
                    to: Some("dog".into()),
                    ..RawRule::default()
                }],
            },
        )
        .unwrap();
        agent.load_persistent_state(state_dir.clone()).unwrap();

        commands.send(AppCommand::SetPaused(true)).unwrap();
        {
            let mut runtime = Runtime::new(&mut agent, None, &receiver);
            runtime.process_pending().unwrap();
            assert_eq!(observed.borrow().change_count_calls, 2);
            assert_eq!(observed.borrow().read_calls, 1);

            {
                let mut state = observed.borrow_mut();
                state.item = Some(ClipboardItem::from_text("cat"));
                state.change_count += 1;
            }
            runtime.process_pending().unwrap();
            assert!(observed.borrow().change_count_calls >= 4);
            assert_eq!(observed.borrow().read_calls, 2);

            commands.send(AppCommand::SetPaused(false)).unwrap();
            runtime.process_pending().unwrap();
        }

        assert_eq!(
            observed
                .borrow()
                .item
                .as_ref()
                .and_then(ClipboardItem::text),
            Some("cat")
        );
        assert!(agent.notifications.delivered.is_empty());
        let snapshot =
            crate::state::LastClipboardSnapshot::load(&state_dir.join("last-clipboard.cbor"))
                .unwrap()
                .unwrap();
        assert_eq!(snapshot.item.text(), Some("cat"));
    }

    #[test]
    fn a_command_wake_does_not_probe_unrelated_clipboard_state() {
        let (commands, receiver) = std::sync::mpsc::channel();
        let (clipboard, observed) = ObservedClipboard::new("bird");
        let mut agent = Agent::new(
            clipboard,
            NoopNotificationBackend::default(),
            ConfigDocument {
                plugins: Default::default(),
                config: AppConfig::default(),
                rules: Vec::new(),
            },
        )
        .unwrap();
        let mut runtime = Runtime::new(&mut agent, None, &receiver);
        let now = Instant::now();
        runtime.process(RuntimeWork::all(), now).unwrap();
        let clipboard_calls = observed.borrow().change_count_calls;

        commands.send(AppCommand::SetPaused(true)).unwrap();
        runtime
            .process(
                RuntimeWork {
                    commands: true,
                    ..RuntimeWork::default()
                },
                now + Duration::from_millis(1),
            )
            .unwrap();

        assert_eq!(observed.borrow().change_count_calls, clipboard_calls);
        assert!(runtime.tray_snapshot().paused);
    }

    #[test]
    fn coalesced_command_wakes_preserve_channel_order() {
        let (commands, receiver) = std::sync::mpsc::channel();
        commands.send(AppCommand::SetPaused(true)).unwrap();
        commands.send(AppCommand::SetPaused(false)).unwrap();
        let mut agent = agent(None);
        let mut runtime = Runtime::new(&mut agent, None, &receiver);

        runtime
            .process(
                RuntimeWork {
                    commands: true,
                    ..RuntimeWork::default()
                },
                Instant::now(),
            )
            .unwrap();

        assert!(!runtime.tray_snapshot().paused);
    }

    #[test]
    fn completed_rule_job_calls_the_wake_sink() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let (_commands, receiver) = std::sync::mpsc::channel();
        let wake_count = Arc::new(AtomicUsize::new(0));
        let worker_count = Arc::clone(&wake_count);
        let mut agent = agent(Some("cat"));
        agent.set_wake_sink(Arc::new(move || {
            worker_count.fetch_add(1, Ordering::Release);
        }));
        let mut runtime = Runtime::new(&mut agent, None, &receiver);
        runtime.process_pending().unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while wake_count.load(Ordering::Acquire) == 0 {
            assert!(
                Instant::now() < deadline,
                "rule worker did not wake the host"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn reliable_clipboard_notifications_remove_the_fallback_deadline() {
        let (_commands, receiver) = std::sync::mpsc::channel();
        let (clipboard, observed) = ObservedClipboard::new("bird");
        let mut agent = Agent::new(
            clipboard,
            NoopNotificationBackend::default(),
            ConfigDocument {
                plugins: Default::default(),
                config: AppConfig::default(),
                rules: Vec::new(),
            },
        )
        .unwrap();
        let mut runtime = Runtime::new(&mut agent, None, &receiver);
        runtime.set_clipboard_notifications(true);
        let now = Instant::now();
        runtime.process(RuntimeWork::all(), now).unwrap();
        assert_eq!(runtime.next_deadline(), None);
        let calls = observed.borrow().change_count_calls;

        observed.borrow_mut().change_count += 1;
        runtime
            .process(
                RuntimeWork {
                    clipboard_changed: true,
                    ..RuntimeWork::default()
                },
                now + Duration::from_secs(30),
            )
            .unwrap();

        assert!(observed.borrow().change_count_calls > calls);
        assert_eq!(runtime.next_deadline(), None);
    }
}
