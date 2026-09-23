//! Turning an agent's stdout into events while it streams.

use abnegate_exec::executor::OutputLimiter;
use abnegate_secret::redact;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::ChildStdout;
use tokio::sync::oneshot;

use crate::event::AgentEvent;
use crate::kind::AgentKind;
use crate::lines::Lines;
use crate::log::Journal;
use crate::log::Record;
use crate::log::Sink;
use crate::stdout_parse_result::StdoutParseResult;

const BUFFER: usize = 8 * 1024;

/// Reads one run's stdout to its end, and asks for the run to be abandoned
/// the moment the agent reports a failure or its output breaks a limit.
pub(crate) struct Reader {
    agent: AgentKind,
    lines: Lines,
    limiter: OutputLimiter,
    limit: usize,
    journal: Journal,
    prose: Sink,
    stop: Option<oneshot::Sender<String>>,
    count: u64,
    events: Vec<AgentEvent>,
    result: StdoutParseResult,
}

impl Reader {
    pub(crate) fn new(
        agent: AgentKind,
        line_limit: usize,
        output_limit: usize,
        journal: Journal,
        prose: Sink,
        stop: oneshot::Sender<String>,
    ) -> Self {
        Self {
            agent,
            lines: Lines::new(line_limit),
            limiter: OutputLimiter::new(output_limit),
            limit: output_limit,
            journal,
            prose,
            stop: Some(stop),
            count: 0,
            events: Vec::new(),
            result: StdoutParseResult::default(),
        }
    }

    pub(crate) async fn run(
        mut self,
        stdout: Option<ChildStdout>,
    ) -> Result<StdoutParseResult, String> {
        let outcome = self.read(stdout).await;
        if let Err(reason) = &outcome {
            self.abandon(reason);
            self.journal
                .append(Record::StdoutFailed, json!({ "error": reason }))
                .await;
        }
        self.journal
            .append(
                Record::StdoutClosed,
                json!({ "line_count": self.count, "finished": self.result.finished }),
            )
            .await;
        self.prose.finish().await;
        outcome.map(|()| self.result)
    }

    async fn read(&mut self, stdout: Option<ChildStdout>) -> Result<(), String> {
        let Some(mut stdout) = stdout else {
            return Ok(());
        };
        let mut buffer = [0_u8; BUFFER];

        loop {
            let read = stdout
                .read(&mut buffer)
                .await
                .map_err(|error| format!("could not read the agent's output: {error}"))?;
            if read == 0 {
                break;
            }
            let (accepted, count, _) = self.limiter.check(read);
            if accepted {
                self.lines.extend(&buffer[..count]);
            }
            while let Some(line) = self.lines.take().map_err(|overlong| overlong.to_string())? {
                self.consume(line).await;
            }
            if !accepted || count < read {
                return Err(format!("the agent's output exceeded {} bytes", self.limit));
            }
        }

        if let Some(line) = self
            .lines
            .flush()
            .map_err(|overlong| overlong.to_string())?
        {
            self.consume(line).await;
        }
        Ok(())
    }

    async fn consume(&mut self, line: String) {
        self.count += 1;
        if self.journal.enabled() {
            self.journal
                .append(
                    Record::StdoutLine,
                    json!({ "line_number": self.count, "line": redact(&line) }),
                )
                .await;
        }

        self.agent.interpret(&line, &mut self.events);
        let mut events = std::mem::take(&mut self.events);
        for event in events.drain(..) {
            match &event {
                AgentEvent::Text(text) => self.prose.write(redact(text).as_bytes()).await,
                AgentEvent::Tool(call) => {
                    tracing::debug!(agent = %self.agent, tool = %call.function.name, "agent tool call");
                }
                AgentEvent::Failed(message) => self.abandon(message),
                _ => {}
            }
            self.result.record(event);
        }
        self.events = events;
    }

    fn abandon(&mut self, reason: &str) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(reason.to_string());
        }
    }
}
