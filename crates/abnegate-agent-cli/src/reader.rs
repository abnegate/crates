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
use crate::overlong_error::OverlongError;
use crate::parser;
use crate::parser::Turn;
use crate::scrubber::Scrubber;
use crate::stdout_parse_result::StdoutParseResult;
use crate::verdict::Verdict;

const BUFFER: usize = 8 * 1024;

/// Reads one run's stdout to its end, and reports a [`Verdict`] the moment
/// the stream settles the run: the agent finished, reported a failure, or its
/// output broke one of the stream's limits.
///
/// An event too long to read is dropped and counted, unless it is one the
/// run cannot do without, which fails it: a tool call or result the size of
/// a file costs nothing, but a result or prose that cannot be read does.
///
/// Only the prose counts against the output limit. The rest of the stream,
/// tool results included, is parsed and dropped, so however long a run goes
/// on it costs no more memory than its answer.
///
/// A [partial message](AgentKind::partial) is counted but never journaled: a
/// secret streamed across several pieces matches none of them, while the
/// whole event that repeats them is scrubbed as one.
///
/// Its lines are read through one [`Turn`], and a turn the account's usage
/// credits carry past the plan's window is logged once, at info.
///
/// [Forwarding](Reader::forwarding) hands each event on as it is read, up to
/// the first failure or usage limit the agent reports, which ends what is
/// handed on and is never itself handed on: a stream reports it as its error.
pub(crate) struct Reader {
    agent: AgentKind,
    turn: Turn,
    lines: Lines,
    limiter: OutputLimiter,
    limit: usize,
    journal: Journal,
    prose: Sink,
    scrubber: Scrubber,
    verdicts: Option<mpsc::Sender<Verdict>>,
    forward: Option<mpsc::UnboundedSender<AgentEvent>>,
    cancel: watch::Receiver<bool>,
    count: u64,
    partials: u64,
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
            turn: agent.turn(),
            lines: Lines::new(line_limit),
            limiter: OutputLimiter::new(output_limit),
            limit: output_limit,
            journal,
            prose,
            scrubber,
            verdicts: Some(verdicts),
            forward: None,
            cancel,
            count: 0,
            partials: 0,
            events: Vec::new(),
            result: StdoutParseResult::default(),
        }
    }

    /// Hand each event on to `forward` as it is read, when given.
    pub(crate) fn forwarding(mut self, forward: Option<mpsc::UnboundedSender<AgentEvent>>) -> Self {
        self.forward = forward;
        self
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
                json!({
                    "line_count": self.count,
                    "partial_line_count": self.partials,
                    "finished": self.result.finished,
                }),
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
            loop {
                match self.lines.take() {
                    Ok(Some(line)) => self.consume(line).await?,
                    Ok(None) => break,
                    Err(overlong) => self.skip(overlong).await?,
                }
            }
        }

        match self.lines.flush() {
            Ok(Some(line)) => self.consume(line).await,
            Ok(None) => Ok(()),
            Err(overlong) => self.skip(overlong).await,
        }
    }

    /// Drop an event too long to read, unless the run cannot do without it.
    async fn skip(&mut self, overlong: OverlongError) -> Result<(), String> {
        self.count += 1;
        if self.agent.essential(&overlong.prefix) {
            return Err(overlong.to_string());
        }
        self.result.dropped += 1;
        let kind = parser::types(&overlong.prefix)
            .first()
            .map(|(_, kind)| (*kind).to_string());
        tracing::warn!(agent = %self.agent, %overlong, kind, "dropped an event too long to read");
        self.journal
            .append_line(
                Record::StdoutDropped,
                json!({ "line_number": self.count, "limit": overlong.limit, "type": kind }),
            )
            .await;
        Ok(())
    }

    async fn consume(&mut self, line: String) -> Result<(), String> {
        self.count += 1;
        if self.agent.partial(&line) {
            self.partials += 1;
        } else if self.journal.enabled() {
            let logged = self.scrubber.scrub(&line);
            self.journal
                .append_line(
                    Record::StdoutLine,
                    json!({ "line_number": self.count, "line": logged }),
                )
                .await;
        }

        self.turn.interpret(&line, &mut self.events);
        let events = std::mem::take(&mut self.events);
        for event in events {
            let event = match event {
                AgentEvent::Text(text) => {
                    if self.limiter.admit(text.len()).accepted < text.len() {
                        return Err(format!("{} {} bytes", crate::PROSE_EXCEEDED, self.limit));
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
                AgentEvent::Limited(limit) => {
                    let message = self.scrubber.scrub(&limit.message).into_owned();
                    self.settle(Verdict::Failed(message.clone()));
                    AgentEvent::Limited(limit.with_message(message))
                }
                AgentEvent::Diagnostic(message) => {
                    let message = self.scrubber.scrub(&message).into_owned();
                    tracing::debug!(agent = %self.agent, diagnostic = %message, "agent diagnostic");
                    AgentEvent::Diagnostic(message)
                }
                AgentEvent::Finished { finish_reason } => {
                    self.settle(Verdict::Finished);
                    AgentEvent::finished(finish_reason)
                }
                AgentEvent::Credits(window) => {
                    tracing::info!(agent = %self.agent, %window, "the turn runs on usage credits past the plan's window");
                    AgentEvent::Credits(window)
                }
                event => event,
            };
            self.hand_on(&event);
            self.result.record(event);
        }
        Ok(())
    }

    fn hand_on(&mut self, event: &AgentEvent) {
        if matches!(event, AgentEvent::Failed(_) | AgentEvent::Limited(_)) {
            self.forward = None;
        }
        if let Some(forward) = &self.forward
            && forward.send(event.clone()).is_err()
        {
            self.forward = None;
        }
    }

    fn settle(&mut self, verdict: Verdict) {
        if let Some(verdicts) = self.verdicts.take() {
            let _ = verdicts.try_send(verdict);
        }
    }
}
