use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::protocol::LogLevel;
use crate::protocol::OutboundMessage;

use super::output_kind::OutputKind;
use super::output_limiter::OutputLimiter;

/// Relays one of a child's output pipes as raw byte chunks.
///
/// Output is bytes, not text: each chunk is base64 on the wire, so invalid
/// UTF-8 and a truncation inside a character are both harmless. The pipe is
/// read to its end whatever happens downstream -- bytes past the limit, or
/// after the receiver has gone, are read and dropped -- because a pipe closed
/// early kills its writer with SIGPIPE. Only cancellation stops the reading.
/// Nothing is read before `RunStarted` has been delivered.
pub(super) struct OutputStream {
    pub(super) job_id: String,
    pub(super) kind: OutputKind,
    pub(super) sender: mpsc::Sender<OutboundMessage>,
    pub(super) limiter: Arc<Mutex<OutputLimiter>>,
    pub(super) buffer_size: usize,
    pub(super) started: watch::Receiver<bool>,
}

impl OutputStream {
    pub(super) fn spawn<R>(self, reader: R, cancellation: CancellationToken) -> JoinHandle<()>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        tokio::spawn(self.relay(reader, cancellation))
    }

    async fn relay<R>(mut self, mut reader: R, cancellation: CancellationToken)
    where
        R: AsyncRead + Unpin,
    {
        tokio::select! {
            _ = self.started.wait_for(|started| *started) => {}
            () = cancellation.cancelled() => return,
        }
        let mut buffer = vec![0; self.buffer_size.max(1)];
        let mut sequence: u64 = 0;
        let mut delivering = true;

        loop {
            let read = tokio::select! {
                read = reader.read(&mut buffer) => read,
                () = cancellation.cancelled() => return,
            };
            let count = match read {
                Ok(0) | Err(_) => return,
                Ok(count) => count,
            };
            if !delivering {
                continue;
            }

            let (accepted, written, first_truncation) = self.admit(count);
            if first_truncation {
                delivering = self
                    .sender
                    .send(OutboundMessage::log(
                        self.job_id.clone(),
                        LogLevel::Warn,
                        format!("Output truncated at {written} bytes"),
                        None,
                    ))
                    .await
                    .is_ok();
            }
            if delivering && accepted > 0 {
                sequence += 1;
                delivering = self
                    .sender
                    .send(self.chunk(&buffer[..accepted], sequence))
                    .await
                    .is_ok();
            }
        }
    }

    /// How many of `count` bytes fit under the shared limit, the total
    /// written once they are counted, and whether this chunk is the first to
    /// lose bytes to the limit.
    fn admit(&self, count: usize) -> (usize, usize, bool) {
        let mut limiter = self.limiter.lock().unwrap_or_else(PoisonError::into_inner);
        let admission = limiter.admit(count);
        (
            admission.accepted,
            limiter.bytes_written(),
            admission.first_truncation,
        )
    }

    fn chunk(&self, bytes: &[u8], sequence: u64) -> OutboundMessage {
        let job_id = self.job_id.clone();
        let data = BASE64_STANDARD.encode(bytes);
        match self.kind {
            OutputKind::Stdout => OutboundMessage::RunStdout {
                job_id,
                data,
                sequence,
            },
            OutputKind::Stderr => OutboundMessage::RunStderr {
                job_id,
                data,
                sequence,
            },
        }
    }
}
