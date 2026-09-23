use std::os::unix::process::ExitStatusExt;
use std::time::Duration;
use std::time::Instant;

use tokio::process::Child;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::protocol::ErrorCode;
use crate::protocol::LogLevel;
use crate::protocol::OutboundMessage;

use super::ending::Ending;
use super::process_group::ProcessGroup;

/// Waits on a spawned child and reports how its run ended.
///
/// A run ends in exactly one terminal message: `RunExit` when the child
/// exits, or `RunError` when it times out or is cancelled. Output the child
/// wrote before it exited is delivered before that message. A descendant
/// that outlives the child and still holds its output open is waited for
/// until the run's deadline, or the grace period if that is later, and then
/// killed with the rest of the group.
pub(super) struct Supervisor {
    pub(super) job_id: String,
    pub(super) sender: mpsc::Sender<OutboundMessage>,
    pub(super) process_group: ProcessGroup,
    pub(super) started_at: Instant,
    pub(super) timeout: Duration,
    pub(super) grace_period: Duration,
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
        self,
        mut child: Child,
        streams: Vec<JoinHandle<()>>,
        cancellation: CancellationToken,
    ) {
        let ending = tokio::select! {
            status = child.wait() => Ending::Exited(status),
            () = tokio::time::sleep(self.timeout) => Ending::TimedOut,
            () = cancellation.cancelled() => Ending::Cancelled,
        };

        match ending {
            Ending::Exited(Ok(status)) => {
                let remaining = self.timeout.saturating_sub(self.started_at.elapsed());
                if !drain(streams, remaining.max(self.grace_period)).await {
                    let _ = self.process_group.kill();
                }
                self.send(OutboundMessage::RunExit {
                    job_id: self.job_id.clone(),
                    exit_code: status.code(),
                    signal: status.signal(),
                    duration_ms: self.elapsed_milliseconds(),
                })
                .await;
            }
            Ending::Exited(Err(error)) => {
                self.send(OutboundMessage::error(
                    self.job_id.clone(),
                    ErrorCode::InternalError,
                    format!("Wait failed: {error}"),
                ))
                .await;
            }
            Ending::TimedOut => {
                let timeout = self.timeout.as_millis();
                self.send(OutboundMessage::log(
                    self.job_id.clone(),
                    LogLevel::Warn,
                    format!("Command timed out after {timeout}ms, killing"),
                    None,
                ))
                .await;
                self.stop(&mut child).await;
                drain(streams, self.grace_period).await;
                self.send(OutboundMessage::error(
                    self.job_id.clone(),
                    ErrorCode::Timeout,
                    format!("Command timed out after {timeout}ms"),
                ))
                .await;
            }
            Ending::Cancelled => {
                self.stop(&mut child).await;
                drain(streams, self.grace_period).await;
                self.send(OutboundMessage::error(
                    self.job_id.clone(),
                    ErrorCode::Cancelled,
                    format!("Command cancelled after {}ms", self.elapsed_milliseconds()),
                ))
                .await;
            }
        }
    }

    /// Ask the whole group to terminate, give the leader the grace period to
    /// exit, then kill whatever is left and reap the leader.
    async fn stop(&self, child: &mut Child) {
        let _ = self.process_group.terminate();
        let _ = tokio::time::timeout(self.grace_period, child.wait()).await;
        let _ = self.process_group.kill();
        let _ = child.wait().await;
    }

    async fn send(&self, message: OutboundMessage) {
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
