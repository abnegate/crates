use std::os::unix::process::ExitStatusExt;
use std::time::Duration;
use std::time::Instant;

use nix::unistd::Pid;
use tokio::process::Child;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::AbortHandle;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::protocol::ErrorCode;
use crate::protocol::LogLevel;
use crate::protocol::OutboundMessage;

use super::ending::Ending;
use super::leader_exit::LeaderExit;
use super::process_group::ProcessGroup;

/// Waits on a spawned child, enforces its deadline and cancellation, and
/// reports how its run ended.
///
/// A run is its process group: when the child exits, or is stopped, every
/// process left in its group is killed before the run is reported, and the
/// child is reaped only after that, so its group's id cannot have been reused
/// by then. A run ends in exactly one terminal message -- `RunExit` when the
/// child exits, `RunError` when it times out, is cancelled, or cannot be
/// watched -- sent after the output that reached its pipes, and never before
/// `RunStarted` has been delivered. Enforcement never waits on the consumer.
pub(super) struct Supervisor {
    pub(super) job_id: String,
    pub(super) sender: mpsc::Sender<OutboundMessage>,
    pub(super) process_group: ProcessGroup,
    pub(super) started_at: Instant,
    pub(super) timeout: Duration,
    pub(super) grace_period: Duration,
    /// Becomes true once `RunStarted` has been delivered.
    pub(super) started: watch::Receiver<bool>,
}

impl Supervisor {
    pub(super) fn spawn(
        self,
        child: Child,
        streams: Vec<JoinHandle<()>>,
        cancellation: CancellationToken,
    ) {
        tokio::spawn(self.supervise(child, streams, cancellation));
    }

    async fn supervise(
        mut self,
        mut child: Child,
        streams: Vec<JoinHandle<()>>,
        cancellation: CancellationToken,
    ) {
        let mut exit = match LeaderExit::watch(Pid::from_raw(self.process_group.pgid())) {
            Ok(exit) => exit,
            Err(error) => {
                self.abandon(&mut child, streams).await;
                self.report(OutboundMessage::error(
                    self.job_id.clone(),
                    ErrorCode::InternalError,
                    format!("Watching the command failed: {error}"),
                ))
                .await;
                return;
            }
        };

        let remaining = self.timeout.saturating_sub(self.started_at.elapsed());
        let ending = tokio::select! {
            exited = exit.wait() => Ending::Exited(exited),
            () = tokio::time::sleep(remaining) => Ending::TimedOut,
            () = cancellation.cancelled() => Ending::Cancelled,
        };

        match ending {
            Ending::Exited(Ok(())) => {
                let _ = self.process_group.kill();
                self.delivered_start().await;
                let drained = tokio::select! {
                    drained = drain(streams, self.grace_period) => Some(drained),
                    () = cancellation.cancelled() => None,
                };
                let status = child.wait().await;
                match (drained, status) {
                    (None, _) => self.report_cancelled().await,
                    (Some(finished), Ok(status)) => {
                        self.warn_unless_drained(finished).await;
                        self.report(OutboundMessage::RunExit {
                            job_id: self.job_id.clone(),
                            exit_code: status.code(),
                            signal: status.signal(),
                            duration_ms: self.elapsed_milliseconds(),
                        })
                        .await;
                    }
                    (Some(_), Err(error)) => {
                        self.report(OutboundMessage::error(
                            self.job_id.clone(),
                            ErrorCode::InternalError,
                            format!("Wait failed: {error}"),
                        ))
                        .await;
                    }
                }
            }
            Ending::Exited(Err(error)) => {
                self.abandon(&mut child, streams).await;
                self.report(OutboundMessage::error(
                    self.job_id.clone(),
                    ErrorCode::InternalError,
                    format!("Watching the command failed: {error}"),
                ))
                .await;
            }
            Ending::TimedOut => {
                self.stop(&mut child, &mut exit).await;
                self.delivered_start().await;
                let finished = drain(streams, self.grace_period).await;
                self.warn_unless_drained(finished).await;
                let timeout = self.timeout.as_millis();
                self.report(OutboundMessage::log(
                    self.job_id.clone(),
                    LogLevel::Warn,
                    format!("Command timed out after {timeout}ms and was killed"),
                    None,
                ))
                .await;
                self.report(OutboundMessage::error(
                    self.job_id.clone(),
                    ErrorCode::Timeout,
                    format!("Command timed out after {timeout}ms"),
                ))
                .await;
            }
            Ending::Cancelled => {
                self.stop(&mut child, &mut exit).await;
                self.delivered_start().await;
                drain(streams, self.grace_period).await;
                self.report_cancelled().await;
            }
        }
    }

    /// Ask the whole group to terminate, give the child the grace period to
    /// exit, then kill whatever is left, and only then reap the child.
    async fn stop(&self, child: &mut Child, exit: &mut LeaderExit) {
        let _ = self.process_group.terminate();
        let _ = tokio::time::timeout(self.grace_period, exit.wait()).await;
        let _ = self.process_group.kill();
        let _ = child.wait().await;
    }

    /// Kill the group, reap the child and stop reading its output.
    async fn abandon(&self, child: &mut Child, streams: Vec<JoinHandle<()>>) {
        let _ = self.process_group.kill();
        let _ = child.wait().await;
        for stream in streams {
            stream.abort();
        }
    }

    async fn delivered_start(&mut self) {
        let _ = self.started.wait_for(|started| *started).await;
    }

    async fn warn_unless_drained(&self, finished: bool) {
        if !finished {
            self.report(OutboundMessage::log(
                self.job_id.clone(),
                LogLevel::Warn,
                format!(
                    "Stopped reading output after {}ms: a process outside the job's group still holds it open",
                    self.grace_period.as_millis()
                ),
                None,
            ))
            .await;
        }
    }

    async fn report_cancelled(&self) {
        self.report(OutboundMessage::error(
            self.job_id.clone(),
            ErrorCode::Cancelled,
            format!("Command cancelled after {}ms", self.elapsed_milliseconds()),
        ))
        .await;
    }

    async fn report(&self, message: OutboundMessage) {
        let _ = self.sender.send(message).await;
    }

    fn elapsed_milliseconds(&self) -> u64 {
        u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// Wait up to `limit` for every stream to reach the end of its pipe, and
/// abandon the ones that do not. Returns whether all of them finished.
async fn drain(streams: Vec<JoinHandle<()>>, limit: Duration) -> bool {
    let aborts: Vec<AbortHandle> = streams.iter().map(JoinHandle::abort_handle).collect();
    let finished = tokio::time::timeout(limit, async {
        for stream in streams {
            let _ = stream.await;
        }
    })
    .await
    .is_ok();
    if !finished {
        for abort in aborts {
            abort.abort();
        }
    }
    finished
}
