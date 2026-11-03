//! One started agent, waited on until its output is read to the end.

use std::time::Duration;

use abnegate_exec::executor::GRACE_PERIOD;
use abnegate_exec::executor::ProcessGroup;
use abnegate_llm::ExitStatus;
use abnegate_llm::ProviderError;
use serde_json::json;
use tempfile::NamedTempFile;
use tokio::process::Child;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio::time::timeout;
use tokio::time::timeout_at;

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
use crate::stdout_parse_result::StdoutParseResult;
use crate::verdict::Verdict;

const UNCLOSED: &str = "the agent's output stayed open after it exited";
const UNSTOPPABLE: &str = "the agent could not be stopped";
const LINGERED: &str = "the agent finished its turn but did not exit";

/// A spawned agent, with the tasks reading its output and everything that
/// has to outlive it, until [`Run::finish`] collects what it did.
///
/// The deadline was set when the agent started, so the timeout covers the
/// whole run however long the caller takes to finish it. Dropped before it
/// finishes, a run kills the agent's whole process group: the reaper is
/// declared ahead of the child, so it drops first, while the group's leader
/// is still unreaped and its id cannot belong to anyone else.
pub(crate) struct Run {
    pub(crate) name: String,
    pub(crate) label: String,
    pub(crate) timeout: Duration,
    pub(crate) deadline: Option<Instant>,
    pub(crate) journal: Journal,
    pub(crate) files: Option<ExecutionLogFiles>,
    pub(crate) reaper: Reaper,
    pub(crate) child: Child,
    pub(crate) writer: JoinHandle<()>,
    pub(crate) reader: JoinHandle<Result<StdoutParseResult, String>>,
    pub(crate) diagnostics: JoinHandle<String>,
    pub(crate) settled: mpsc::Receiver<Verdict>,
    pub(crate) cancel: watch::Sender<bool>,
    /// Held only so the MCP file the agent reads outlives it.
    pub(crate) _mcp: Option<McpAttachment>,
    /// Held only so the instructions file the agent reads outlives it.
    pub(crate) _instructions: Option<NamedTempFile>,
}

impl Run {
    /// Wait for the agent to exit, or for its output to settle the run, and
    /// report everything it did.
    ///
    /// Only a run that succeeded leaves behind what the agent forked. Any
    /// other takes the agent's whole process group with it. A run that fails
    /// here still reports where its logs are, with every line read before it
    /// failed flushed to them.
    pub(crate) async fn finish(mut self) -> Result<Execution, ExecutionError> {
        let deadline = self.deadline;
        let expiry = async move {
            match deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };
        let outcome = tokio::select! {
            biased;
            status = self.child.wait() => Outcome::Exited(status),
            Some(verdict) = self.settled.recv() => Outcome::Settled(verdict),
            () = expiry => Outcome::TimedOut,
        };

        let (status, stopped, failure) = match self.settle(outcome).await {
            Ok(settlement) => settlement,
            Err(error) => {
                self.writer.abort();
                let _ = drain(&mut self.reader, self.reaper.group(), &self.cancel).await;
                let _ = drain(&mut self.diagnostics, self.reaper.group(), &self.cancel).await;
                return Err(ExecutionError::new(error, self.files.take()));
            }
        };
        let status = ExitStatus::from(status);
        self.journal
            .append(Record::Exited, json!({ "status": status.to_string() }))
            .await;

        self.writer.abort();
        let stdout = match drain(&mut self.reader, self.reaper.group(), &self.cancel).await {
            Ok(Ok(stdout)) => stdout,
            Ok(Err(message)) | Err(message) => {
                let _ = drain(&mut self.diagnostics, self.reaper.group(), &self.cancel).await;
                return Err(ExecutionError::new(
                    ProviderError::malformed(&self.name, message),
                    self.files.take(),
                ));
            }
        };
        let stderr = match drain(&mut self.diagnostics, self.reaper.group(), &self.cancel).await {
            Ok(stderr) => stderr,
            Err(message) => {
                return Err(ExecutionError::new(
                    ProviderError::malformed(&self.name, message),
                    self.files.take(),
                ));
            }
        };
        let failure = failure.or_else(|| {
            std::iter::from_fn(|| self.settled.try_recv().ok()).find_map(|verdict| match verdict {
                Verdict::Failed(reason) => Some(reason),
                Verdict::Finished => None,
            })
        });
        if status == ExitStatus::Code(0)
            && stdout.finished
            && stdout.failure.is_none()
            && stopped.is_none()
        {
            self.reaper.disarm();
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
                    "failure": reported,
                    "stopped": stopped,
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

    /// Carry the wait through to an exit status, stopping the agent when the
    /// wait ended without one, and say why it was stopped when it was and
    /// what failure settled the run when one did.
    async fn settle(
        &mut self,
        outcome: Outcome,
    ) -> Result<(std::process::ExitStatus, Option<String>, Option<String>), ProviderError> {
        let verdict = match outcome {
            Outcome::Exited(Ok(status)) => return Ok((status, None, None)),
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
                return Err(ProviderError::timeout(&self.name, self.timeout));
            }
            Outcome::Settled(verdict) => verdict,
        };

        let (reason, failure) = match verdict {
            Verdict::Finished => (LINGERED.to_string(), None),
            Verdict::Failed(reason) => {
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
        if let Ok(Ok(status)) = timeout_at(patience, self.child.wait()).await {
            return Ok((status, None, failure));
        }

        let status = stop_agent(&mut self.child, self.reaper.group())
            .await
            .ok_or_else(|| ProviderError::malformed(&self.name, UNSTOPPABLE))?;
        self.journal
            .append(
                Record::Stopped,
                json!({ "exit_code": status.code(), "reason": reason }),
            )
            .await;
        Ok((status, Some(reason), failure))
    }
}

/// Terminate the agent's group, and kill whatever is left of it once the
/// grace period runs out.
///
/// A group is identified by its leader's process id, which the system may
/// hand to a new process once the leader is reaped and no member is left.
/// So the group is killed while the leader is still unreaped whenever it
/// outlives the grace period. When the leader exits within it, reaping comes
/// first, and the kill that follows reaches stragglers safely only because a
/// straggler still alive keeps the group's id from being reused; a group
/// that empties in the instant between the two leaves the kill to land on
/// whatever took the id, a race this cannot close without a process handle
/// the platform does not offer here.
async fn stop_agent(
    child: &mut Child,
    group: Option<&ProcessGroup>,
) -> Option<std::process::ExitStatus> {
    let Some(group) = group else {
        let _ = child.start_kill();
        return child.wait().await.ok();
    };
    let _ = group.terminate();
    if let Ok(Ok(status)) = timeout(GRACE_PERIOD, child.wait()).await {
        let _ = group.kill_until_gone().await;
        return Some(status);
    }
    let _ = group.kill_until_gone().await;
    child.wait().await.ok()
}

/// A reader's result once the agent has exited.
///
/// A descendant that outlived the agent can still hold its output open, and a
/// reader waiting for that end of file would wait forever. Such stragglers get
/// the grace period to finish, then the rest of the group is killed, and a
/// reader still held open by something outside the group is told to stop with
/// what it has.
async fn drain<T>(
    task: &mut JoinHandle<T>,
    group: Option<&ProcessGroup>,
    cancel: &watch::Sender<bool>,
) -> Result<T, String> {
    if let Ok(joined) = timeout(GRACE_PERIOD, &mut *task).await {
        return joined.map_err(|error| error.to_string());
    }
    if let Some(group) = group {
        let _ = group.kill_until_gone().await;
    }
    if let Ok(joined) = timeout(GRACE_PERIOD, &mut *task).await {
        return joined.map_err(|error| error.to_string());
    }
    tracing::warn!("{UNCLOSED}; keeping what was read");
    let _ = cancel.send(true);
    task.await.map_err(|error| error.to_string())
}
