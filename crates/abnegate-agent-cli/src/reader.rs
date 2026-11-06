//! Turning an agent's stdout into events while it streams.

use std::borrow::Cow;
use std::path::PathBuf;

use abnegate_exec::executor::OutputLimiter;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::ChildStdout;
use tokio::sync::mpsc;
use tokio::sync::watch;

use crate::drain::Drain;
use crate::event::AgentEvent;
use crate::kind::AgentKind;
use crate::lines::Lines;
use crate::log::Journal;
use crate::log::Record;
use crate::log::Sink;
use crate::overlong_error::OverlongError;
use crate::paragraphs::Paragraphs;
use crate::parser;
use crate::parser::Turn;
use crate::scrubber::Scrubber;
use crate::settings::DEFAULT_TOOL_OUTPUT_LIMIT;
use crate::stdout_parse_result::StdoutParseResult;
use crate::subagent::Subagent;
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
/// Only the prose counts against the output limit. The reader keeps only
/// the run's answer, the main agent's tool calls and what the result
/// reports; the rest of the stream is parsed and dropped. What a tool printed
/// and news of a subagent are handed on and never kept, their text scrubbed
/// of the run's secrets first, and a tool's output then cut to the tool
/// output limit. What is handed on waits in the stream until it is read, so a
/// caller that falls behind holds every event it has not read yet, each tool
/// output no longer than that limit.
///
/// A [partial message](AgentKind::partial) is counted but never journaled: a
/// secret streamed across several pieces matches none of them, while the
/// whole event that repeats them is scrubbed as one.
///
/// Its lines are read through one [`Turn`], and a turn the account's usage
/// credits carry past the plan's window is logged once, at info, with the
/// run's working directory.
///
/// Every block of prose after the first starts with a paragraph break, which
/// counts against the output limit like the rest of it. The final answer is
/// left as the agent gave it.
///
/// [Forwarding](Reader::forwarding) hands each event on as it is read, up to
/// the first failure or usage limit the agent reports, which ends what is
/// handed on and is never itself handed on: a stream reports it as its error.
///
/// Once the run starts to [`Drain`] it, the reader still takes everything the
/// pipe holds, and stops when the drain ends.
pub(crate) struct Reader {
    agent: AgentKind,
    turn: Turn,
    paragraphs: Paragraphs,
    directory: Option<PathBuf>,
    lines: Lines,
    limiter: OutputLimiter,
    limit: usize,
    printed: usize,
    journal: Journal,
    prose: Sink,
    scrubber: Scrubber,
    verdicts: Option<mpsc::Sender<Verdict>>,
    forward: Option<mpsc::UnboundedSender<AgentEvent>>,
    drain: Drain,
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
        draining: watch::Receiver<bool>,
    ) -> Self {
        Self {
            agent,
            turn: agent.turn(),
            paragraphs: Paragraphs::default(),
            directory: None,
            lines: Lines::new(line_limit),
            limiter: OutputLimiter::new(output_limit),
            limit: output_limit,
            printed: DEFAULT_TOOL_OUTPUT_LIMIT,
            journal,
            prose,
            scrubber,
            verdicts: Some(verdicts),
            forward: None,
            drain: Drain::new(draining),
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

    /// Hand on at most `limit` bytes of what one tool printed.
    pub(crate) fn printing(mut self, limit: usize) -> Self {
        self.printed = limit;
        self
    }

    /// Name `directory` as the run's working directory in what it logs, or
    /// this process's own when there is none.
    pub(crate) fn in_directory(mut self, directory: Option<PathBuf>) -> Self {
        self.directory = directory;
        self
    }

    /// Everything read, including when the drain ended the read before the
    /// output closed.
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
            let Some(read) = self.drain.read(stdout.read(&mut buffer)).await else {
                return Ok(());
            };
            let read =
                read.map_err(|error| format!("could not read the agent's output: {error}"))?;
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
            let event = match self.paragraphs.separated(event) {
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
                AgentEvent::Output(mut output) => {
                    scrub(&self.scrubber, &mut output.text);
                    AgentEvent::Output(output.truncated(self.printed))
                }
                AgentEvent::Subagent(subagent) => {
                    AgentEvent::Subagent(scrubbed(&self.scrubber, subagent))
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
                    let directory = self
                        .directory
                        .clone()
                        .or_else(|| std::env::current_dir().ok())
                        .map(|directory| {
                            self.scrubber
                                .scrub(&directory.display().to_string())
                                .into_owned()
                        });
                    tracing::info!(
                        agent = %self.agent,
                        %window,
                        directory = directory.as_deref().map(tracing::field::display),
                        "the turn runs on usage credits past the plan's window"
                    );
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

/// `text`, with whatever `scrubber` removes from it removed.
fn scrub(scrubber: &Scrubber, text: &mut String) {
    if let Cow::Owned(scrubbed) = scrubber.scrub(text) {
        *text = scrubbed;
    }
}

/// `subagent`, with every word it carries scrubbed.
fn scrubbed(scrubber: &Scrubber, mut subagent: Subagent) -> Subagent {
    let words = [
        &mut subagent.description,
        &mut subagent.kind,
        &mut subagent.model,
        &mut subagent.last_tool,
        &mut subagent.summary,
        &mut subagent.error,
    ];
    for text in words.into_iter().flatten() {
        scrub(scrubber, text);
    }
    if let Some(step) = &mut subagent.step {
        scrub(scrubber, &mut step.name);
        if let Some(subject) = &mut step.subject {
            scrub(scrubber, subject);
        }
    }
    subagent
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use tempfile::TempDir;
    use tokio::sync::mpsc;
    use tokio::sync::watch;

    use super::Reader;
    use crate::kind::AgentKind;
    use crate::log::Journal;
    use crate::log::Sink;
    use crate::scrubber::Scrubber;
    use crate::settings::DEFAULT_LINE_LIMIT;
    use crate::settings::DEFAULT_OUTPUT_LIMIT;
    use crate::test_support::ESCAPED_PROCESS;
    use crate::test_support::PATIENCE;
    use crate::test_support::kill_escaped;
    use crate::test_support::shell;

    /// Few enough of [`TEXT`] that they all fit in a pipe nobody reads yet.
    const WRITTEN: usize = 100;
    const TEXT: &str = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"a"}]}}"#;
    const RESULT: &str = r#"{"type":"result","subtype":"success","is_error":false}"#;

    /// A reader still behind when the run gave up on the output closing
    /// stopped at once, losing everything the agent wrote before it exited.
    #[tokio::test]
    async fn what_the_agent_wrote_before_the_drain_started_is_read_to_its_end() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = format!(
            "{ESCAPED_PROCESS}i=0; while [ $i -lt {WRITTEN} ]; do echo '{TEXT}'; i=$((i + 1)); done\necho '{RESULT}'\n"
        );
        let mut agent = shell(directory.path(), &script)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the agent");
        let stdout = agent.stdout.take();
        agent.wait().await.expect("the agent to exit");
        let (verdicts, _settled) = mpsc::channel(2);
        let (draining, started) = watch::channel(false);
        draining.send_replace(true);
        let reader = Reader::new(
            AgentKind::Claude,
            DEFAULT_LINE_LIMIT,
            DEFAULT_OUTPUT_LIMIT,
            Journal::disabled(),
            Sink::open(None).await,
            Scrubber::new([]),
            verdicts,
            started,
        );

        let read = tokio::time::timeout(PATIENCE, reader.run(stdout)).await;
        kill_escaped(directory.path());

        let result = read
            .expect("the drain to end while an escaped process holds the output open")
            .expect("the output to be read");
        assert_eq!(
            result.text,
            ["a"; WRITTEN].join("\n\n"),
            "output buffered before the drain started was dropped"
        );
        assert!(result.finished, "the buffered result was dropped");
    }
}
