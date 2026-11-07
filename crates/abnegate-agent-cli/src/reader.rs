//! Turning an agent's stdout into events while it streams.

use std::borrow::Cow;
use std::path::PathBuf;

use abnegate_exec::executor::OutputLimiter;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::ChildStdout;
use tokio::sync::mpsc;
use tokio::sync::watch;

use crate::activity::ELLIPSIS;
use crate::drain::Drain;
use crate::event::AgentEvent;
use crate::kind::AgentKind;
use crate::lines::Lines;
use crate::log::Journal;
use crate::log::Record;
use crate::log::Sink;
use crate::overlong_error::OverlongError;
use crate::paragraphs::PARAGRAPH_BREAK;
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
/// When [awaiting background work](Reader::awaiting_background), a finished
/// turn settles nothing while the agent reports work running in the background,
/// and the turn that work leads to is read like the first. A turn that begins
/// after a finished one settled the run reopens it, so the run waits again.
///
/// A turn that begins after one finished, and is still unfinished when the
/// output ends, settles the run and is noted on the result, which the run
/// marks [`CUT_SHORT`](crate::CUT_SHORT) once it knows nothing failed. Answers
/// joined across turns are held to the output limit, as prose is.
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
    verdicts: Option<mpsc::UnboundedSender<Verdict>>,
    forward: Option<mpsc::UnboundedSender<AgentEvent>>,
    drain: Drain,
    await_background: bool,
    /// Whether a verdict has settled the run.
    settled: bool,
    /// Whether the verdict that settled it was a finished turn a later one
    /// may reopen.
    reopenable: bool,
    /// How many turns have finished.
    finished: u64,
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
        verdicts: mpsc::UnboundedSender<Verdict>,
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
            await_background: false,
            settled: false,
            reopenable: false,
            finished: 0,
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

    /// Let a finished turn settle the run only once nothing the agent sent to
    /// the background is left to come, when `awaited`.
    pub(crate) fn awaiting_background(mut self, awaited: bool) -> Self {
        self.await_background = awaited;
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
        if outcome.is_ok() && self.turn_open() && self.result.failure.is_none() {
            self.result.end_mid_turn();
            self.settle(Verdict::Finished);
        }
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
        let begun = self.turn.turns_begun();
        self.turn.begin_unread(&overlong.prefix);
        if self.turn.turns_begun() > begun {
            self.reopen();
        }
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

        let begun = self.turn.turns_begun();
        self.turn.interpret(&line, &mut self.events);
        if self.turn.turns_begun() > begun {
            self.reopen();
        }
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
                AgentEvent::Answer(answer) => {
                    let joined = self
                        .result
                        .earlier_answer_len()
                        .map_or(answer.len(), |earlier| {
                            earlier + PARAGRAPH_BREAK.len() + answer.len()
                        });
                    if joined > self.limit {
                        return Err(format!("{} {} bytes", crate::PROSE_EXCEEDED, self.limit));
                    }
                    AgentEvent::Answer(answer)
                }
                AgentEvent::Finished { finish_reason } => {
                    self.finished += 1;
                    if self.agent == AgentKind::Claude && self.finished > self.turn.turns_begun() {
                        tracing::debug!(
                            agent = %self.agent,
                            results = self.finished,
                            begun = self.turn.turns_begun(),
                            "more results than turns begun: claude wrote no init for one"
                        );
                    }
                    if !(self.await_background
                        && (self.turn.background_running() || self.turn_open()))
                    {
                        self.settle(Verdict::Finished);
                    }
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
        if self.settled && !(self.reopenable && verdict != Verdict::Finished) {
            return;
        }
        self.settled = true;
        self.reopenable = self.await_background && verdict == Verdict::Finished;
        let verdicts = if self.await_background {
            self.verdicts.clone()
        } else {
            self.verdicts.take()
        };
        if let Some(verdicts) = verdicts {
            let _ = verdicts.send(verdict);
        }
    }

    /// Whether a turn began after one finished and has not finished itself.
    /// Claude can begin a turn before writing the result of the one before,
    /// so it is the count of turns begun and finished that says so.
    fn turn_open(&self) -> bool {
        self.finished > 0 && self.turn.turns_begun() > self.finished
    }

    /// Say the run must wait again, when a finished turn settled it and the
    /// agent has begun another.
    fn reopen(&mut self) {
        if !self.reopenable {
            return;
        }
        self.settled = false;
        self.reopenable = false;
        if let Some(verdicts) = &self.verdicts {
            let _ = verdicts.send(Verdict::Reopened);
        }
    }
}

/// `text`, with whatever `scrubber` removes from it removed.
fn scrub(scrubber: &Scrubber, text: &mut String) {
    if let Cow::Owned(scrubbed) = scrubber.scrub(text) {
        *text = scrubbed;
    }
}

/// `text`, cut short as a subject is, with whatever `scrubber` removes from
/// it removed, a piece of a secret left at the cut included.
fn scrub_cut(scrubber: &Scrubber, text: &mut String) {
    if let Cow::Owned(scrubbed) = scrubber.scrub_cut(text, ELLIPSIS) {
        *text = scrubbed;
    }
}

/// `subagent`, with every word it carries scrubbed.
fn scrubbed(scrubber: &Scrubber, mut subagent: Subagent) -> Subagent {
    if let Some(description) = &mut subagent.description {
        scrub_cut(scrubber, description);
    }
    let words = [
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
            scrub_cut(scrubber, subject);
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

    use abnegate_secret::SecretValue;

    use super::Reader;
    use super::scrubbed;
    use crate::activity::subject;
    use crate::kind::AgentKind;
    use crate::log::Journal;
    use crate::log::Sink;
    use crate::scrubber::Scrubber;
    use crate::settings::DEFAULT_LINE_LIMIT;
    use crate::settings::DEFAULT_OUTPUT_LIMIT;
    use crate::subagent::StepStatus;
    use crate::subagent::Subagent;
    use crate::subagent::SubagentStep;
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
    /// A subject is cut before the run's secrets are known to it, so the
    /// cut can leave the start of one, which the reader then removes.
    #[test]
    fn a_piece_of_a_secret_a_subjects_cut_left_is_never_handed_on() {
        const TOKEN: &str = "relay-token-0123456789abcdef0123456789abcdef";
        let scrubber = Scrubber::new([SecretValue::new(TOKEN)]);
        let command = format!(
            "deploy --target staging --region eu-west-1 --verbose --dry-run --name {TOKEN} --wait"
        );
        let cut =
            subject(&serde_json::json!({ "command": command }).to_string()).expect("a subject");
        assert!(cut.ends_with("--name relay-toke\u{2026}"), "{cut}");
        let step = SubagentStep::new("toolu_02", "Bash", StepStatus::Running).with_subject(cut);
        let update = Subagent::new("toolu_01")
            .with_description(format!("{}\u{2026}", &TOKEN[..20]))
            .with_step(step);

        let handed = scrubbed(&scrubber, update);

        let subject = handed
            .step
            .and_then(|step| step.subject)
            .expect("a subject");
        assert!(subject.ends_with("--name \u{2026}"), "{subject}");
        let description = handed.description.expect("a description");
        assert_eq!(description, "\u{2026}");
    }

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
        let (verdicts, _settled) = mpsc::unbounded_channel();
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
