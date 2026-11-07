//! One started agent, waited on until its output is read to the end.

use std::time::Duration;

use abnegate_exec::executor::GRACE_PERIOD;
use abnegate_exec::executor::ProcessGroup;
use abnegate_llm::ExitStatus;
use abnegate_llm::ProviderError;
use serde_json::json;
use tempfile::NamedTempFile;
use tempfile::TempDir;
use tokio::process::Child;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio::time::timeout;
use tokio::time::timeout_at;

use crate::CUT_SHORT;
use crate::cleanup::Cleanup;
use crate::event::AgentEvent;
use crate::execution::Execution;
use crate::execution_error::ExecutionError;
use crate::log::EXECUTION_LOG_PREVIEW_LIMIT;
use crate::log::ExecutionLogFiles;
use crate::log::Journal;
use crate::log::Record;
use crate::log::preview;
use crate::mcp::McpAttachment;
use crate::outcome::Outcome;
use crate::reaper::Reaper;
use crate::rollout::Rollout;
use crate::settlement::Settlement;
use crate::stdout_parse_result::StdoutParseResult;
use crate::verdict::Verdict;

const UNSTOPPABLE: &str = "the agent could not be stopped";
const LINGERED: &str = "the agent finished its turn but did not exit";
const OUTLASTED: &str = "the run's timeout came while the agent was still at work";

/// A spawned agent, with the tasks reading its output and everything that
/// has to outlive it, until [`Run::finish`] collects what it did.
///
/// The deadline was set when the agent started, so the timeout covers the
/// agent however long the caller takes to finish the run. Dropped before it
/// finishes, a run kills the agent's whole process group and everything that
/// carries its marker: the reaper is declared ahead of the child, so it
/// drops first, while the group's leader is still unreaped and its id cannot
/// belong to anyone else, as it did in the [`Spawned`](crate::spawned::Spawned)
/// the run was built from.
pub(crate) struct Run {
    pub(crate) name: String,
    pub(crate) label: String,
    pub(crate) timeout: Duration,
    pub(crate) deadline: Option<Instant>,
    pub(crate) cleanup: Cleanup,
    pub(crate) journal: Journal,
    pub(crate) files: Option<ExecutionLogFiles>,
    pub(crate) reaper: Reaper,
    pub(crate) child: Child,
    pub(crate) writer: JoinHandle<()>,
    pub(crate) reader: JoinHandle<Result<StdoutParseResult, String>>,
    pub(crate) diagnostics: JoinHandle<String>,
    pub(crate) settled: mpsc::UnboundedReceiver<Verdict>,
    /// Turned true when a turn finished while the run waited on background
    /// work, so the agent has answered even if it never settles.
    pub(crate) answered: watch::Receiver<bool>,
    /// Turned true once the run stops waiting for the agent's output to
    /// close, which starts its readers' drain.
    pub(crate) draining: watch::Sender<bool>,
    /// Where a Codex agent keeps the rollout of its session, read once it
    /// has exited by itself for the window its turn ran in.
    pub(crate) rollout: Option<Rollout>,
    /// Where a stream's events go, which the window read from the rollout
    /// follows last.
    pub(crate) forward: Option<mpsc::UnboundedSender<AgentEvent>>,
    /// Held only so the MCP file the agent reads outlives it.
    pub(crate) _mcp: Option<McpAttachment>,
    /// Held only so the instructions file the agent reads outlives it.
    pub(crate) _instructions: Option<NamedTempFile>,
    /// Held only so the image files the agent opens outlive it, and are
    /// removed once the run ends.
    pub(crate) _images: Option<TempDir>,
}

impl Run {
    /// Wait for the agent to exit, or for its output to settle the run, and
    /// report everything it did.
    ///
    /// The agent is reaped only once the run has cleaned up after it, so
    /// every kill of its group lands while the group's id is still its own.
    /// What the agent left behind is stopped as the run's [`Cleanup`] says:
    /// after any run but a successful one by default, the agent's exit
    /// status, read while it is unreaped, included. A run that fails here
    /// still reports where its logs are, with every line read before it
    /// failed flushed to them.
    ///
    /// A Codex agent that exited by itself, and named its session, has the
    /// window its turn ran in read from its rollout, folded into its stdout
    /// and handed on last to a stream. A run that timed out, that the run
    /// stopped, or whose diagnostics tripped its tripwire, reads nothing,
    /// whether or not the agent also reported a failure of its own.
    pub(crate) async fn finish(mut self) -> Result<Execution, ExecutionError> {
        let settlement = loop {
            let outcome = self.outcome().await;
            match self.settle(outcome).await {
                Ok(None) => {}
                Ok(Some(settlement)) => break Ok(settlement),
                Err(error) => break Err(error),
            }
        };
        let unstopped = settlement
            .as_ref()
            .is_ok_and(|settled| settled.stopped.is_none());
        if !unstopped || self.cleanup == Cleanup::Always {
            self.reaper.clean().await;
            self.draining.send_replace(true);
        }
        self.writer.abort();
        let Settlement {
            stopped,
            failure,
            mut tripped,
            exit,
        } = match settlement {
            Ok(settlement) => settlement,
            Err(error) => {
                let _ = drain(&mut self.reader, self.reaper.group(), &self.draining).await;
                let _ = drain(&mut self.diagnostics, self.reaper.group(), &self.draining).await;
                let _ = self.reap().await;
                return Err(ExecutionError::new(error, self.files.take()));
            }
        };

        let mut stdout = match drain(&mut self.reader, self.reaper.group(), &self.draining).await {
            Ok(Ok(stdout)) => stdout,
            Ok(Err(message)) | Err(message) => {
                let _ = drain(&mut self.diagnostics, self.reaper.group(), &self.draining).await;
                self.reaper.clean().await;
                let _ = self.reap().await;
                return Err(ExecutionError::new(
                    ProviderError::malformed(&self.name, message),
                    self.files.take(),
                ));
            }
        };
        if stopped.as_deref() == Some(OUTLASTED)
            && failure.is_none()
            && stdout.failure.is_none()
            && stdout.finish_reason.as_deref() != Some(CUT_SHORT)
        {
            let cut_short = AgentEvent::finished(Some(CUT_SHORT.to_string()));
            if let Some(forward) = &self.forward {
                let _ = forward.send(cut_short.clone());
            }
            stdout.record(cut_short);
        }
        let stderr = match drain(&mut self.diagnostics, self.reaper.group(), &self.draining).await {
            Ok(stderr) => stderr,
            Err(message) => {
                self.reaper.clean().await;
                let _ = self.reap().await;
                return Err(ExecutionError::new(
                    ProviderError::malformed(&self.name, message),
                    self.files.take(),
                ));
            }
        };
        let mut failure = failure;
        while let Ok(verdict) = self.settled.try_recv() {
            tripped |= matches!(verdict, Verdict::Tripped(_));
            if failure.is_none() {
                failure = verdict.failure();
            }
        }
        let succeeded = stdout.finished
            && stdout.failure.is_none()
            && stopped.is_none()
            && exit.is_some_and(|status| status.success());
        if !succeeded {
            self.reaper.clean().await;
        }

        let status = match self.reap().await {
            Ok(status) => status,
            Err(error) => {
                self.journal
                    .append(Record::WaitFailed, json!({ "error": error.to_string() }))
                    .await;
                let message = match stopped {
                    Some(_) => UNSTOPPABLE.to_string(),
                    None => error.to_string(),
                };
                return Err(ExecutionError::new(
                    ProviderError::malformed(&self.name, message),
                    self.files.take(),
                ));
            }
        };
        if let Some(reason) = &stopped {
            self.journal
                .append(
                    Record::Stopped,
                    json!({ "exit_code": status.code(), "reason": reason }),
                )
                .await;
        }
        let status = ExitStatus::from(status);
        self.journal
            .append(Record::Exited, json!({ "status": status.to_string() }))
            .await;

        if stopped.is_none()
            && !tripped
            && let (Some(rollout), Some(thread)) = (self.rollout.take(), stdout.session.clone())
            && let Some(occupancy) = rollout.occupancy(thread).await
        {
            let event = AgentEvent::Occupancy(occupancy);
            if let Some(forward) = &self.forward {
                let _ = forward.send(event.clone());
            }
            stdout.record(event);
        }

        let reported = stdout
            .failure
            .as_deref()
            .or(failure.as_deref())
            .map(|reported| preview(reported, EXECUTION_LOG_PREVIEW_LIMIT));
        self.journal
            .append(
                Record::Completed,
                json!({
                    "status": status.to_string(),
                    "stdout_bytes": stdout.text.len(),
                    "stderr_bytes": stderr.len(),
                    "finished": stdout.finished,
                    "has_structured_result": stdout.structured.is_some(),
                    "has_answer": stdout.answer.is_some(),
                    "failure": reported,
                    "stopped": stopped,
                    "occupancy": stdout.occupancy,
                }),
            )
            .await;

        Ok(Execution {
            stdout,
            stderr,
            status,
            stopped,
            failure,
            log: self.files,
        })
    }

    /// Whatever ends the wait first: the agent's exit, a verdict on its
    /// output, or the run's deadline.
    async fn outcome(&mut self) -> Outcome {
        let deadline = self.deadline;
        let expiry = async move {
            match deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            exit = exited(&mut self.child, self.reaper.group()) => Outcome::Exited(exit),
            Some(verdict) = self.settled.recv() => Outcome::Settled(verdict),
            () = expiry => Outcome::TimedOut,
        }
    }

    /// Carry the wait through to the agent's exit, stopping the agent when
    /// the wait ended without one, and say why it was stopped when it was,
    /// what failure settled the run when one did, and the status it exited
    /// with when it exited by itself. The agent is left unreaped.
    async fn settle(&mut self, outcome: Outcome) -> Result<Option<Settlement>, ProviderError> {
        let verdict = match outcome {
            Outcome::Exited(Ok(exit)) => {
                return Ok(Some(Settlement {
                    stopped: None,
                    failure: None,
                    tripped: false,
                    exit,
                }));
            }
            Outcome::Exited(Err(error)) => {
                self.journal
                    .append(Record::WaitFailed, json!({ "error": error.to_string() }))
                    .await;
                stop_agent(&mut self.child, self.reaper.group()).await;
                return Err(ProviderError::malformed(&self.name, error));
            }
            Outcome::TimedOut => {
                self.journal
                    .append(
                        Record::TimedOut,
                        json!({ "timeout_seconds": self.timeout.as_secs_f64() }),
                    )
                    .await;
                tracing::warn!(provider = %self.name, label = %self.label, "agent timed out; stopping its process group");
                stop_agent(&mut self.child, self.reaper.group()).await;
                if *self.answered.borrow() {
                    return Ok(Some(Settlement {
                        stopped: Some(OUTLASTED.to_string()),
                        failure: None,
                        tripped: false,
                        exit: None,
                    }));
                }
                return Err(ProviderError::timeout(&self.name, self.timeout));
            }
            Outcome::Settled(Verdict::Reopened) => return Ok(None),
            Outcome::Settled(verdict) => verdict,
        };

        let finished = verdict == Verdict::Finished;
        let mut tripped = matches!(verdict, Verdict::Tripped(_));
        let (reason, mut failure) = match verdict.failure() {
            None => (LINGERED.to_string(), None),
            Some(reason) => {
                let reason = preview(&reason, EXECUTION_LOG_PREVIEW_LIMIT);
                self.journal
                    .append(Record::Abandoned, json!({ "reason": reason }))
                    .await;
                tracing::warn!(provider = %self.name, label = %self.label, "the run failed while the agent was running; stopping it");
                (reason.clone(), Some(reason))
            }
        };

        let grace = Instant::now() + GRACE_PERIOD;
        let patience = self.deadline.map_or(grace, |deadline| deadline.min(grace));
        let exited_in_grace = loop {
            tokio::select! {
                biased;
                exit = timeout_at(patience, exited(&mut self.child, self.reaper.group())) => {
                    break exit.ok().and_then(Result::ok);
                }
                Some(later) = self.settled.recv() => match later {
                    Verdict::Reopened if finished && failure.is_none() && !tripped => {
                        return Ok(None);
                    }
                    Verdict::Reopened | Verdict::Finished => {}
                    Verdict::Failed(_) | Verdict::Tripped(_) => {
                        tripped |= matches!(later, Verdict::Tripped(_));
                        if failure.is_none() {
                            failure = later.failure();
                        }
                    }
                },
            }
        };
        if let Some(exit) = exited_in_grace {
            return Ok(Some(Settlement {
                stopped: None,
                failure,
                tripped,
                exit,
            }));
        }

        stop_agent(&mut self.child, self.reaper.group()).await;
        Ok(Some(Settlement {
            stopped: Some(reason),
            failure,
            tripped,
            exit: None,
        }))
    }

    /// Release the agent's group, which no kill reaches from here on, and
    /// reap the agent.
    async fn reap(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.reaper.release();
        self.child.wait().await
    }
}

/// Resolve once the agent has exited, with the status it exited with,
/// leaving it unreaped while it leads a group, so that the group's id stays
/// its own until the run has cleaned up after it.
async fn exited(
    child: &mut Child,
    group: Option<&ProcessGroup>,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    match group {
        Some(group) => group.leader_exited().await.map_err(std::io::Error::other),
        None => child.wait().await.map(Some),
    }
}

/// Terminate the agent's group, and kill whatever is left of it once the
/// agent has exited or the grace period has run out.
///
/// The agent is left unreaped, so its group's id cannot pass to another
/// group while the group is signalled. When the agent's exit cannot be
/// watched, it may not be this process's unreaped child, and its group's id
/// may not be its own, so only the agent is killed.
async fn stop_agent(child: &mut Child, group: Option<&ProcessGroup>) {
    let Some(group) = group else {
        let _ = child.start_kill();
        return;
    };
    let _ = group.terminate();
    if let Ok(Err(error)) = timeout(GRACE_PERIOD, group.leader_exited()).await {
        tracing::warn!(%error, "the agent's exit could not be watched; killing the agent alone");
        let _ = child.start_kill();
        return;
    }
    let _ = group.kill_until_gone().await;
}

/// A reader's result once the agent has exited.
///
/// A descendant that outlived the agent can still hold its output open, and a
/// reader waiting for that end of file would wait forever. Such stragglers get
/// the grace period to finish, then the rest of the group is killed and the
/// readers start to [`Drain`](crate::drain::Drain): each still takes what the
/// pipe holds, and stops once it is quiet, or at the cutoff while something
/// outside the group keeps writing. A run that already cleaned up after the
/// agent started the drain as it killed the group, so no straggler is left to
/// wait for.
async fn drain<T>(
    task: &mut JoinHandle<T>,
    group: Option<&ProcessGroup>,
    draining: &watch::Sender<bool>,
) -> Result<T, String> {
    if let Ok(joined) = timeout(GRACE_PERIOD, &mut *task).await {
        return joined.map_err(|error| error.to_string());
    }
    if let Some(group) = group {
        let _ = group.kill_until_gone().await;
    }
    draining.send_replace(true);
    task.await.map_err(|error| error.to_string())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::process::Stdio;

    use abnegate_exec::executor::ProcessGroup;
    use tokio::io::AsyncBufReadExt;
    use tokio::io::BufReader;
    use tokio::process::Command;

    use super::stop_agent;
    use crate::test_support::PATIENCE;
    use crate::test_support::stop_if_running;

    /// Leaves a process leading a group of its own, ignoring SIGTERM, that is
    /// the shell's child rather than the test's, prints its pid, and becomes
    /// a long sleep.
    const STRANGER: &str = concat!(
        r#"perl -e '$| = 1; $SIG{TERM} = q(IGNORE); setpgrp(0, 0); print qq($$\n); sleep 300' &"#,
        "\nexec sleep 300",
    );

    /// Linux cannot watch the exit of a process that is not this process's
    /// child, so nothing says the group's id is still the stranger's own.
    #[tokio::test]
    async fn an_agent_whose_exit_cannot_be_watched_is_killed_without_its_group() {
        let mut agent = Command::new("sh")
            .args(["-c", STRANGER])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("the agent starts");
        let mut lines = BufReader::new(agent.stdout.take().expect("its stdout")).lines();
        let stranger: u32 = lines
            .next_line()
            .await
            .expect("the agent's output reads")
            .expect("the stranger's pid")
            .trim()
            .parse()
            .expect("a pid");
        let group = ProcessGroup::try_from(stranger).expect("a process leading its own group");

        tokio::time::timeout(PATIENCE, stop_agent(&mut agent, Some(&group)))
            .await
            .expect("the agent is stopped");

        let agent_exit = tokio::time::timeout(PATIENCE, agent.wait()).await;
        let spared = stop_if_running(stranger);
        assert!(
            agent_exit.is_ok_and(|status| status.is_ok()),
            "the agent was not killed"
        );
        assert!(
            spared,
            "a group whose leader could not be watched was killed by its id"
        );
    }
}
