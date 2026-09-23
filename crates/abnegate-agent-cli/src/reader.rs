//! Turning an agent's stdout into events while it streams.

use abnegate_exec::executor::OutputLimiter;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::ChildStdout;
use tokio::sync::mpsc;
use tokio::sync::watch;

use crate::event::AgentEvent;
use crate::kind::AgentKind;
use crate::lines::Lines;
use crate::log::Journal;
use crate::log::Record;
use crate::log::Sink;
use crate::scrubber::Scrubber;
use crate::stdout_parse_result::StdoutParseResult;
use crate::verdict::Verdict;

const BUFFER: usize = 8 * 1024;

/// Reads one run's stdout to its end, and reports a [`Verdict`] the moment
/// the stream settles the run: the agent finished, reported a failure, or its
/// output broke one of the stream's limits.
///
/// Only the prose counts against the output limit. The rest of the stream,
/// tool results included, is parsed and dropped, so however long a run goes
/// on it costs no more memory than its answer.
pub(crate) struct Reader {
    agent: AgentKind,
    lines: Lines,
    limiter: OutputLimiter,
    limit: usize,
    journal: Journal,
    prose: Sink,
    scrubber: Scrubber,
    verdicts: Option<mpsc::Sender<Verdict>>,
    cancel: watch::Receiver<bool>,
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
        scrubber: Scrubber,
        verdicts: mpsc::Sender<Verdict>,
        cancel: watch::Receiver<bool>,
    ) -> Self {
        Self {
            agent,
            lines: Lines::new(line_limit),
            limiter: OutputLimiter::new(output_limit),
            limit: output_limit,
            journal,
            prose,
            scrubber,
            verdicts: Some(verdicts),
            cancel,
            count: 0,
            events: Vec::new(),
            result: StdoutParseResult::default(),
        }
    }

    /// Everything read, including when the read was cancelled part way.
    pub(crate) async fn run(
        mut self,
        stdout: Option<ChildStdout>,
    ) -> Result<StdoutParseResult, String> {
        let outcome = self.read(stdout).await;
        self.result.conclude();
        if let Err(reason) = &outcome {
            self.settle(Verdict::Failed(reason.clone()));
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
            let read = tokio::select! {
                biased;
                _ = self.cancel.changed() => return Ok(()),
                read = stdout.read(&mut buffer) => read
                    .map_err(|error| format!("could not read the agent's output: {error}"))?,
            };
            if read == 0 {
                break;
            }
            self.lines.extend(&buffer[..read]);
            while let Some(line) = self.lines.take().map_err(|overlong| overlong.to_string())? {
                self.consume(line).await?;
            }
        }

        if let Some(line) = self
            .lines
            .flush()
            .map_err(|overlong| overlong.to_string())?
        {
            self.consume(line).await?;
        }
        Ok(())
    }

    async fn consume(&mut self, line: String) -> Result<(), String> {
        self.count += 1;
        if self.journal.enabled() {
            let logged = self.scrubber.scrub(&line);
            self.journal
                .append(
                    Record::StdoutLine,
                    json!({ "line_number": self.count, "line": logged }),
                )
                .await;
        }

        self.agent.interpret(&line, &mut self.events);
        let events = std::mem::take(&mut self.events);
        for event in events {
            let event = match event {
                AgentEvent::Text(text) => {
                    let (accepted, count, _) = self.limiter.check(text.len());
                    if !accepted || count < text.len() {
                        return Err(format!("the agent's prose exceeded {} bytes", self.limit));
                    }
                    self.prose
                        .write(self.scrubber.scrub(&text).as_bytes())
                        .await;
                    AgentEvent::Text(text)
                }
                AgentEvent::Tool(call) => {
                    tracing::debug!(agent = %self.agent, tool = %call.function.name, "agent tool call");
                    AgentEvent::Tool(call)
                }
                AgentEvent::Failed(message) => {
                    let message = self.scrubber.scrub(&message).into_owned();
                    self.settle(Verdict::Failed(message.clone()));
                    AgentEvent::Failed(message)
                }
                AgentEvent::Diagnostic(message) => {
                    let message = self.scrubber.scrub(&message).into_owned();
                    tracing::debug!(agent = %self.agent, diagnostic = %message, "agent diagnostic");
                    AgentEvent::Diagnostic(message)
                }
                AgentEvent::Finished { finish_reason } => {
                    self.settle(Verdict::Finished);
                    AgentEvent::Finished { finish_reason }
                }
                event => event,
            };
            self.result.record(event);
        }
        Ok(())
    }

    fn settle(&mut self, verdict: Verdict) {
        if let Some(verdicts) = self.verdicts.take() {
            let _ = verdicts.try_send(verdict);
        }
    }
}
