use std::collections::BTreeSet;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use ct_clipboard::ClipboardItem;
use ct_core::{RuleEngine, TransformResult};

enum RuleWorkerCommand {
    Apply {
        job_id: u64,
        input: Box<ClipboardItem>,
        disabled_rule_ids: BTreeSet<String>,
        reply: Option<Sender<RuleWorkerOutcome>>,
    },
    ReplaceEngine(RuleEngine),
    Shutdown,
}

pub type RuleWorkerOutcome = std::result::Result<Option<TransformResult>, String>;

pub struct RuleWorkerCompletion {
    pub job_id: u64,
    pub outcome: RuleWorkerOutcome,
}

pub struct RuleWorker {
    commands: Sender<RuleWorkerCommand>,
    completions: Receiver<RuleWorkerCompletion>,
    thread: Option<JoinHandle<()>>,
}

impl RuleWorker {
    pub fn start(mut engine: RuleEngine) -> Result<Self> {
        let (command_sender, commands) = mpsc::channel();
        let (completion_sender, completions) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("clipboard-transformer-rules".to_string())
            .spawn(move || {
                while let Ok(command) = commands.recv() {
                    match command {
                        RuleWorkerCommand::Apply {
                            job_id,
                            input,
                            disabled_rule_ids,
                            reply,
                        } => {
                            let outcome = engine
                                .try_apply_owned_excluding(*input, &disabled_rule_ids)
                                .map_err(|error| format!("{error:#}"));
                            if let Some(reply) = reply {
                                let _ = reply.send(outcome);
                            } else if completion_sender
                                .send(RuleWorkerCompletion { job_id, outcome })
                                .is_err()
                            {
                                break;
                            }
                        }
                        RuleWorkerCommand::ReplaceEngine(replacement) => engine = replacement,
                        RuleWorkerCommand::Shutdown => break,
                    }
                }
            })
            .context("start rule engine worker")?;
        Ok(Self {
            commands: command_sender,
            completions,
            thread: Some(thread),
        })
    }

    pub fn submit(
        &self,
        job_id: u64,
        input: ClipboardItem,
        disabled_rule_ids: BTreeSet<String>,
    ) -> Result<()> {
        self.commands
            .send(RuleWorkerCommand::Apply {
                job_id,
                input: Box::new(input),
                disabled_rule_ids,
                reply: None,
            })
            .map_err(|_| anyhow!("rule engine worker is unavailable"))
    }

    pub fn apply_blocking(
        &self,
        input: ClipboardItem,
        disabled_rule_ids: BTreeSet<String>,
    ) -> Result<Option<TransformResult>> {
        let (reply_sender, reply) = mpsc::channel();
        self.commands
            .send(RuleWorkerCommand::Apply {
                job_id: 0,
                input: Box::new(input),
                disabled_rule_ids,
                reply: Some(reply_sender),
            })
            .map_err(|_| anyhow!("rule engine worker is unavailable"))?;
        reply
            .recv()
            .map_err(|_| anyhow!("rule engine worker stopped before replying"))?
            .map_err(anyhow::Error::msg)
    }

    pub fn replace_engine(&self, engine: RuleEngine) -> Result<()> {
        self.commands
            .send(RuleWorkerCommand::ReplaceEngine(engine))
            .map_err(|_| anyhow!("rule engine worker is unavailable"))
    }

    pub fn try_completion(&self) -> Result<Option<RuleWorkerCompletion>> {
        match self.completions.try_recv() {
            Ok(completion) => Ok(Some(completion)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(anyhow!(
                "rule engine worker completion channel disconnected"
            )),
        }
    }

    /// Blocks until the next completion arrives, or returns `Ok(None)` once
    /// `timeout` elapses.
    ///
    /// The host loop polls with [`Self::try_completion`] because it has other
    /// work between results. Callers with nothing to do until the worker
    /// replies should wait on the channel instead of spinning against a
    /// wall-clock deadline, which turns scheduling latency on a loaded machine
    /// into a spurious failure.
    pub fn wait_completion(&self, timeout: Duration) -> Result<Option<RuleWorkerCompletion>> {
        match self.completions.recv_timeout(timeout) {
            Ok(completion) => Ok(Some(completion)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(anyhow!(
                "rule engine worker completion channel disconnected"
            )),
        }
    }
}

impl Drop for RuleWorker {
    fn drop(&mut self) {
        let _ = self.commands.send(RuleWorkerCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
