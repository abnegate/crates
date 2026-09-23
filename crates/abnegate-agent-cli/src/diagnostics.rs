//! Draining an agent's stderr.

use abnegate_exec::executor::OutputLimiter;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::ChildStderr;
use tokio::sync::mpsc;
use tokio::sync::watch;

use crate::lines::Lines;
use crate::log::Journal;
use crate::log::Record;
use crate::log::Sink;
use crate::scrubber::Scrubber;
use crate::verdict::Verdict;

const BUFFER: usize = 8 * 1024;

/// Reads one run's stderr to its end, keeping up to the output limit.
///
/// Stderr is drained whether or not it is ever read back, and past the limit
/// too. An agent writing to a piped stderr that nobody reads blocks the moment
/// it fills the pipe buffer, which on a verbose agent happens long before it
/// reaches its answer.
pub(crate) struct Diagnostics {
    lines: Lines,
    framing: bool,
    limiter: OutputLimiter,
    journal: Journal,
    raw: Sink,
    scrubber: Scrubber,
    tripwire: Option<fn(&str) -> bool>,
    verdicts: Option<mpsc::Sender<Verdict>>,
    cancel: watch::Receiver<bool>,
    count: u64,
    collected: Vec<u8>,
}

impl Diagnostics {
    pub(crate) fn new(
        line_limit: usize,
        output_limit: usize,
        journal: Journal,
        raw: Sink,
        scrubber: Scrubber,
        tripwire: Option<fn(&str) -> bool>,
        verdicts: mpsc::Sender<Verdict>,
        cancel: watch::Receiver<bool>,
    ) -> Self {
        Self {
            lines: Lines::new(line_limit),
            framing: true,
            limiter: OutputLimiter::new(output_limit),
            journal,
            raw,
            scrubber,
            tripwire,
            verdicts: Some(verdicts),
            cancel,
            count: 0,
            collected: Vec::new(),
        }
    }

    /// Everything kept, including when the read was cancelled part way.
    pub(crate) async fn run(mut self, stderr: Option<ChildStderr>) -> String {
        if let Some(stderr) = stderr {
            self.read(stderr).await;
        }

        self.journal
            .append(Record::StderrClosed, json!({ "line_count": self.count }))
            .await;
        self.raw.finish().await;
        self.scrubber
            .scrub(&String::from_utf8_lossy(&self.collected))
            .into_owned()
    }

    async fn read(&mut self, mut stderr: ChildStderr) {
        let mut buffer = [0_u8; BUFFER];
        loop {
            let read = tokio::select! {
                biased;
                _ = self.cancel.changed() => return,
                read = stderr.read(&mut buffer) => read,
            };
            let Ok(read) = read else {
                break;
            };
            if read == 0 {
                break;
            }
            let (accepted, count, _) = self.limiter.check(read);
            if accepted {
                self.keep(&buffer[..count]).await;
            }
        }
        if self.framing
            && let Ok(Some(line)) = self.lines.flush()
        {
            self.line(line).await;
        }
    }

    async fn keep(&mut self, chunk: &[u8]) {
        self.collected.extend_from_slice(chunk);
        if !self.framing {
            return;
        }
        self.lines.extend(chunk);
        loop {
            match self.lines.take() {
                Ok(Some(line)) => self.line(line).await,
                Ok(None) => break,
                Err(overlong) => {
                    tracing::warn!(%overlong, "stopped logging the agent's stderr line by line");
                    self.framing = false;
                    break;
                }
            }
        }
    }

    async fn line(&mut self, line: String) {
        self.count += 1;
        let scrubbed = self.scrubber.scrub(&line).into_owned();
        if self.tripwire.is_some_and(|tripwire| tripwire(&line))
            && let Some(verdicts) = self.verdicts.take()
        {
            let _ = verdicts.try_send(Verdict::Failed(scrubbed.clone()));
        }

        tracing::debug!(line = %scrubbed, "agent stderr");
        self.raw.write(scrubbed.as_bytes()).await;
        self.raw.write(b"\n").await;
        if self.journal.enabled() {
            self.journal
                .append(
                    Record::StderrLine,
                    json!({ "line_number": self.count, "line": scrubbed }),
                )
                .await;
        }
    }
}
