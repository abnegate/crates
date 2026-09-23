//! Draining an agent's stderr.

use abnegate_exec::executor::OutputLimiter;
use abnegate_secret::redact;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::ChildStderr;

use crate::lines::Lines;
use crate::log::Journal;
use crate::log::Record;
use crate::log::Sink;

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
    count: u64,
    collected: Vec<u8>,
}

impl Diagnostics {
    pub(crate) fn new(line_limit: usize, output_limit: usize, journal: Journal, raw: Sink) -> Self {
        Self {
            lines: Lines::new(line_limit),
            framing: true,
            limiter: OutputLimiter::new(output_limit),
            journal,
            raw,
            count: 0,
            collected: Vec::new(),
        }
    }

    pub(crate) async fn run(mut self, stderr: Option<ChildStderr>) -> String {
        if let Some(mut stderr) = stderr {
            let mut buffer = [0_u8; BUFFER];
            while let Ok(read) = stderr.read(&mut buffer).await {
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

        self.journal
            .append(Record::StderrClosed, json!({ "line_count": self.count }))
            .await;
        self.raw.finish().await;
        redact(&String::from_utf8_lossy(&self.collected)).into_owned()
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
        let line = redact(&line);
        tracing::debug!(line = %line, "agent stderr");
        self.raw.write(line.as_bytes()).await;
        self.raw.write(b"\n").await;
        if self.journal.enabled() {
            self.journal
                .append(
                    Record::StderrLine,
                    json!({ "line_number": self.count, "line": line }),
                )
                .await;
        }
    }
}
