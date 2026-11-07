use std::collections::BTreeMap;
use std::time::Duration;

use abnegate_llm::Limit;
use abnegate_llm::ToolCall;
use abnegate_llm::Usage;
use abnegate_llm::Window;
use chrono::DateTime;
use chrono::Utc;
use serde_json::Value;

use crate::event::AgentEvent;
use crate::occupancy::Occupancy;
use crate::parser;
use crate::parser::claude::CREDITS_UNCONFIRMED;
use crate::parser::claude::cli_content_block::CliContentBlock;
use crate::parser::claude::cli_message::CliMessage;
use crate::parser::claude::cli_usage::CliUsage;
use crate::parser::claude::credits_refusal::CreditsRefusal;
use crate::parser::claude::lifecycle::Lifecycle;
use crate::parser::claude::model_usage::ModelUsage;
use crate::parser::claude::notice::Notice;
use crate::parser::claude::rate_limit_report::OVERAGE_INCLUDED_WINDOW;
use crate::parser::claude::rate_limit_report::RateLimitReport;
use crate::parser::claude::refusal::Refusal;
use crate::parser::claude::refusal::limited;
use crate::parser::claude::reply::Reply;
use crate::parser::claude::report::Report;
use crate::parser::claude::roster::Roster;
use crate::parser::claude::salvage;
use crate::parser::claude::stream_event::StreamEvent;
use crate::parser::claude::task_event::TaskEvent;

/// The wording a throttled run is reported with, followed by the refused
/// window and its status in brackets.
///
/// A caller recognises a throttled run by the words in the failure, so this
/// phrase is load-bearing and not decoration.
const THROTTLED: &str = "rate limit reached";

const FAILED: &str = "the agent reported a failed run";

const FAILED_SUBTYPE_PREFIX: &str = "error";

const NO_ARGUMENTS: &str = "{}";

const UNNAMED_WINDOW: &str = "request";

/// The system notice claude opens a session with, naming it.
const INIT_SUBTYPE: &str = "init";

/// The kind of line that tells of the session or of a task claude runs.
const SYSTEM: &str = "system";

/// The `system` line that lists all that runs in the background.
const BACKGROUND_TASKS_SUBTYPE: &str = "background_tasks_changed";

/// The kind of line that reports a command claude runs on its own account.
const COMMAND_LIFECYCLE: &str = "command_lifecycle";

/// The tool that sets, or with [`WAKEUP_STOP`] cancels, the wakeup that starts
/// a later turn.
const SCHEDULE_WAKEUP: &str = "ScheduleWakeup";

/// The [`SCHEDULE_WAKEUP`] argument that cancels the wakeup.
const WAKEUP_STOP: &str = "stop";

/// The kind of line that hands the model back what its tools printed.
const USER: &str = "user";

/// Why claude could not look the account's usage credits up, which a later
/// attempt may get past.
const INDETERMINATE_REASONS: [&str; 2] = ["fetch_error", "unknown"];

/// The status of a request a limit refused.
const TOO_MANY_REQUESTS: u16 = 429;

/// The status of a request from an account that is not signed in, which no
/// wording makes a limit.
const UNAUTHORIZED: u16 = 401;

/// One turn of `claude --output-format stream-json`, read a line at a time.
///
/// A line's meaning can hang on the lines before it. claude reports a
/// refused request on a `rate_limit_event` that names no agent, then writes
/// the refusal as the line of the agent whose request it was, so the window
/// is remembered and limits the turn only on the main agent's API error or a
/// failed result. A line the main agent writes past it shows its own request
/// went through, and the window was a subagent's.
///
/// A subagent's words, its token counts and its API errors are its own
/// conversation's, and claude hands its failure to the main agent as the
/// result of the call that started it. The main agent's call that starts a
/// subagent, with Claude's Agent tool or the Task tool of older releases, is
/// one of its [tools](AgentEvent::Tool), followed by the
/// [subagent](AgentEvent::Subagent)'s start; every call the subagent makes,
/// or an agent nested inside it, is a step of the subagent's run, and claude's
/// `task_*` notices and the call's report say how it goes and how it ends. A
/// call by an agent no known subagent descends from is the main agent's.
/// Every tool result is the [output](AgentEvent::Output) of the call it
/// answers, a subagent's included.
///
/// A `system` or `user` line is read straight into the shape this crate
/// needs, and a tool's account of its call no further than a subagent's
/// report goes; every other line is read as a [`StreamEvent`].
///
/// The session is announced once, from the first `init` notice, though
/// claude repeats its id on other notices and on the result.
///
/// The result's `modelUsage` names each model's context window, and the
/// turn's [`AgentEvent::Occupancy`] is the window of the model the main agent
/// last wrote on, filled as far as its latest request filled it, or else the
/// widest window the result names, with nothing said of its fill.
#[derive(Debug, Clone, Default)]
pub struct Turn {
    /// The latest reason claude gave for the account not spending usage
    /// credits.
    reason: Option<String>,
    /// The latest refused window, as the limit it would be.
    refusal: Option<Refusal>,
    /// When the latest refused request's window resets, which a limit that
    /// ends the turn reports.
    resets_at: Option<DateTime<Utc>>,
    /// The latest refused request's window, which a limit that ends the turn
    /// reports.
    window: Option<Window>,
    /// The main agent's failed request, which the result after it repeats.
    failure: Option<AgentEvent>,
    /// Whether the turn has said it runs on usage credits.
    credited: bool,
    /// Whether the turn has announced its session.
    announced: bool,
    /// The model the main agent last wrote on.
    model: Option<String>,
    /// The tokens the main agent's latest request filled.
    used: Option<u64>,
    /// The calls waiting on their results and the subagents the main agent
    /// started.
    roster: Roster,
    /// Whether the latest notice of what runs in the background listed any.
    background: bool,
    /// The main agent's call that set the wakeup still to fire.
    wakeup: Option<String>,
}

impl Turn {
    /// Whether work runs in the background or a wakeup has yet to fire.
    pub(crate) fn background_running(&self) -> bool {
        self.background || self.wakeup.is_some()
    }

    /// Translate one line of the stream, appending whatever it means beside
    /// the lines before it.
    pub fn interpret(&mut self, line: &str, events: &mut Vec<AgentEvent>) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        match parser::kind(line) {
            Some(SYSTEM) => match serde_json::from_str::<Notice>(line) {
                Ok(notice) => self.notice(notice, events),
                Err(_) => salvage(line, events),
            },
            Some(USER) => match serde_json::from_str::<Reply>(line) {
                Ok(reply) => {
                    self.returned(reply.message, reply.tool_use_result.as_ref(), events);
                }
                Err(_) => salvage(line, events),
            },
            Some(COMMAND_LIFECYCLE) => {
                if serde_json::from_str::<Lifecycle>(line).is_ok_and(|lifecycle| lifecycle.woke()) {
                    self.wakeup = None;
                }
            }
            _ => match serde_json::from_str::<StreamEvent>(line) {
                Ok(event) => self.read(event, events),
                Err(_) => salvage(line, events),
            },
        }
    }

    pub(crate) fn read(&mut self, event: StreamEvent, events: &mut Vec<AgentEvent>) {
        match event {
            StreamEvent::System {
                subtype,
                session_id,
            } => self.announce(subtype.as_deref(), session_id, events),
            StreamEvent::Assistant {
                message,
                parent_tool_use_id: Some(parent),
                ..
            } => self.roster.delegated(&parent, calls(message), events),
            StreamEvent::Assistant {
                message,
                parent_tool_use_id: None,
                is_api_error_message,
                api_error,
            } => self.assistant(message, is_api_error_message, api_error.as_deref(), events),
            StreamEvent::Result {
                subtype,
                is_error,
                result,
                errors,
                structured_output,
                total_cost_usd,
                turns,
                api_milliseconds,
                usage,
                api_error_status,
                model_usage,
                ..
            } => {
                events.extend(structured_output.map(AgentEvent::Structured));
                events.extend(total_cost_usd.map(AgentEvent::Cost));
                events.extend(
                    turns
                        .and_then(|turns| u32::try_from(turns).ok())
                        .map(AgentEvent::Turns),
                );
                events.extend(
                    api_milliseconds
                        .and_then(|milliseconds| u64::try_from(milliseconds).ok())
                        .map(|milliseconds| {
                            AgentEvent::Latency(Duration::from_millis(milliseconds))
                        }),
                );
                if let Some(usage) = usage {
                    events.push(AgentEvent::Usage(Usage::from(&usage)));
                    events.push(AgentEvent::Tokens(usage));
                }
                events.extend(self.occupancy(model_usage).map(AgentEvent::Occupancy));
                if failed(subtype.as_deref(), is_error) {
                    events.push(self.failure(subtype, explained(result, errors), api_error_status));
                } else {
                    events.extend(result.filter(|words| spoken(words)).map(AgentEvent::Answer));
                    events.push(AgentEvent::finished(subtype));
                }
            }
            StreamEvent::RateLimit { report, resets_at } => {
                self.limit(report.unwrap_or_default(), resets_at, events);
            }
            StreamEvent::User {
                message,
                tool_use_result,
            } => {
                let report = tool_use_result.as_ref().and_then(Report::from_value);
                self.returned(message, report.as_ref(), events);
            }
            StreamEvent::Unknown => {}
        }
    }

    /// A `user` line's tool results, a wakeup that could not be set forgotten.
    fn returned(
        &mut self,
        message: Option<CliMessage>,
        report: Option<&Report>,
        events: &mut Vec<AgentEvent>,
    ) {
        let results: Vec<(String, Value, bool)> = results(message).collect();
        if results
            .iter()
            .any(|(id, _, failed)| *failed && self.wakeup.as_ref() == Some(id))
        {
            self.wakeup = None;
        }
        self.roster.returned(results, report, events);
    }

    /// A `system` line: the session's opening notice, or news of a task.
    fn notice(&mut self, notice: Notice, events: &mut Vec<AgentEvent>) {
        let Some(subtype) = notice.subtype.as_deref() else {
            return;
        };
        if subtype == INIT_SUBTYPE {
            self.announce(Some(INIT_SUBTYPE), notice.session_id, events);
            return;
        }
        if subtype == BACKGROUND_TASKS_SUBTYPE {
            if let Some(tasks) = &notice.tasks {
                self.background = !tasks.is_empty();
            }
            return;
        }
        let Some(event) = TaskEvent::named(subtype) else {
            return;
        };
        events.extend(self.roster.noticed(event, notice).map(AgentEvent::Subagent));
    }

    fn announce(
        &mut self,
        subtype: Option<&str>,
        session: Option<String>,
        events: &mut Vec<AgentEvent>,
    ) {
        if self.announced || subtype != Some(INIT_SUBTYPE) {
            return;
        }
        if let Some(session) = session.filter(|session| !session.is_empty()) {
            self.announced = true;
            events.push(AgentEvent::Session(session));
        }
    }

    /// Note a wakeup the main agent set or cancelled in `message`.
    fn schedule(&mut self, message: &CliMessage) {
        for block in &message.content {
            if let CliContentBlock::ToolUse { id, name, input } = block
                && name == SCHEDULE_WAKEUP
            {
                let cancelled = input.get(WAKEUP_STOP).and_then(Value::as_bool) == Some(true);
                self.wakeup = (!cancelled).then(|| id.clone());
            }
        }
    }

    fn assistant(
        &mut self,
        message: Option<CliMessage>,
        api_error_message: bool,
        api_error: Option<&str>,
        events: &mut Vec<AgentEvent>,
    ) {
        let kind = api_error.and_then(CreditsRefusal::named);
        if kind.is_some() || api_error_message {
            let words = message.as_ref().map(CliMessage::words).unwrap_or_default();
            let ended = match kind {
                Some(kind) => Some(self.unfunded(kind.marker(), &words)),
                None => self
                    .refusal
                    .as_ref()
                    .map(|refusal| refusal.limited(worded(&refusal.message, &words))),
            };
            if let Some(ended) = ended {
                self.failure = Some(ended.clone());
                events.push(ended);
            }
            return;
        }
        self.refusal = None;
        self.resets_at = None;
        self.window = None;
        let Some(message) = message else {
            return;
        };
        self.schedule(&message);
        if let Some(model) = message.model.filter(|model| !model.is_empty()) {
            if self.model.as_ref() != Some(&model) {
                self.used = None;
            }
            self.model = Some(model);
        }
        if let Some(usage) = &message.usage {
            self.used = Some(filled(usage));
        }
        for block in message.content {
            match block {
                CliContentBlock::Text { text } => events.push(AgentEvent::Text(text)),
                CliContentBlock::ToolUse { id, name, input } => {
                    self.roster.called(call(id, name, &input), events);
                }
                CliContentBlock::ToolResult { .. } | CliContentBlock::Other => {}
            }
        }
        if let Some(usage) = message.usage {
            events.push(AgentEvent::Usage(Usage::from(&usage)));
        }
    }

    /// The window of the model the main agent last wrote on, filled as far
    /// as its latest request filled it, or else the widest window `usage`
    /// names, with nothing said of its fill.
    fn occupancy(&self, usage: Option<BTreeMap<String, ModelUsage>>) -> Option<Occupancy> {
        let usage = usage?;
        let main = self
            .model
            .as_deref()
            .and_then(|model| usage.get(model))
            .and_then(|entry| entry.context_window)
            .and_then(|window| Occupancy::reported(window, self.used));
        main.or_else(|| {
            usage
                .values()
                .filter_map(|entry| entry.context_window)
                .max()
                .and_then(|window| Occupancy::reported(window, None))
        })
    }

    /// What a failed result ends the turn in: the main agent's failed
    /// request when it had one, or else claude's `words` for the result,
    /// falling back to its `subtype`.
    fn failure(
        &self,
        subtype: Option<String>,
        words: Option<String>,
        status: Option<u16>,
    ) -> AgentEvent {
        if let Some(failure) = &self.failure {
            return failure.clone();
        }
        if status == Some(UNAUTHORIZED) {
            return AgentEvent::Failed(
                words.unwrap_or_else(|| subtype.unwrap_or_else(|| FAILED.to_string())),
            );
        }
        match (words, &self.refusal) {
            (Some(words), Some(refusal)) => refusal.limited(worded(&refusal.message, &words)),
            (Some(words), None) => self.judged(words, status),
            (None, Some(refusal)) => refusal.limited(refusal.message.clone()),
            (None, None) => self.judged(subtype.unwrap_or_else(|| FAILED.to_string()), status),
        }
    }

    fn limit(
        &mut self,
        mut report: RateLimitReport,
        resets_at: Option<Value>,
        events: &mut Vec<AgentEvent>,
    ) {
        if report.resets_at.is_none() {
            report.resets_at = resets_at;
        }
        self.reason.clone_from(&report.overage_disabled_reason);
        if report.is_using_overage == Some(true) && !self.credited {
            self.credited = true;
            let window = report.kind.as_deref().unwrap_or(UNNAMED_WINDOW);
            events.push(AgentEvent::Credits(window.to_string()));
        }
        if report.allowed() {
            events.extend(report.measured().map(AgentEvent::Window));
            return;
        }
        self.resets_at = report.resets_at();
        self.window = report.refused();
        if report.error_code.is_some() || report.kind.as_deref() == Some(OVERAGE_INCLUDED_WINDOW) {
            return;
        }
        let message = throttled(&report);
        events.push(AgentEvent::Diagnostic(message.clone()));
        self.refusal = Some(Refusal {
            message,
            resets_at: self.resets_at,
            window: self.window.clone(),
        });
    }

    /// The turn refused past a limit, in `message`, with whatever claude
    /// said of the window it refused on.
    fn limited(&self, message: String, credits: bool) -> AgentEvent {
        limited(message, credits, self.resets_at, self.window.as_ref())
    }

    /// A failed result no refused window accounts for, which is a limit when
    /// claude's status or its words say one refused the turn.
    fn judged(&self, message: String, status: Option<u16>) -> AgentEvent {
        if status == Some(TOO_MANY_REQUESTS) || Limit::worded(&message) {
            return self.limited(message, false);
        }
        AgentEvent::Failed(message)
    }

    /// claude's `words` for a turn usage credits could not fund, begun with
    /// `marker`, or a failure begun with [`CREDITS_UNCONFIRMED`] when claude
    /// could not look the account's usage credits up for the request it
    /// refused.
    fn unfunded(&self, marker: &str, words: &str) -> AgentEvent {
        match self.reason.as_deref() {
            Some(reason) if INDETERMINATE_REASONS.contains(&reason) => {
                AgentEvent::Failed(worded(&format!("{CREDITS_UNCONFIRMED} ({reason})"), words))
            }
            _ => self.limited(worded(marker, words), true),
        }
    }
}

/// Whether a result ended the turn in failure, by its flag or its subtype.
fn failed(subtype: Option<&str>, is_error: bool) -> bool {
    is_error || subtype.is_some_and(|subtype| subtype.starts_with(FAILED_SUBTYPE_PREFIX))
}

/// Every token a request sent and its answer took: the share of the window
/// the request filled.
fn filled(usage: &CliUsage) -> u64 {
    usage
        .prompt_tokens()
        .saturating_add(usage.output_tokens.unwrap_or_default())
}

/// Whether `words` say anything at all.
fn spoken(words: &str) -> bool {
    !words.trim().is_empty()
}

/// claude's words for a failed result: its `result` text, then each of its
/// `errors` on a line of its own, a value that is not a sentence written as
/// JSON, leaving out whatever is blank.
fn explained(result: Option<String>, errors: Option<Vec<Value>>) -> Option<String> {
    let errors = errors
        .into_iter()
        .flatten()
        .filter(|error| !error.is_null())
        .map(|error| match error {
            Value::String(message) => message,
            other => other.to_string(),
        });
    let lines: Vec<String> = result
        .into_iter()
        .chain(errors)
        .filter(|line| spoken(line))
        .map(|line| line.trim().to_string())
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// The calls a subagent's line makes: of everything in its own
/// conversation, only what it reaches for is the turn's.
fn calls(message: Option<CliMessage>) -> impl Iterator<Item = ToolCall> {
    message
        .into_iter()
        .flat_map(|message| message.content)
        .filter_map(|block| match block {
            CliContentBlock::ToolUse { id, name, input } => Some(call(id, name, &input)),
            CliContentBlock::Text { .. }
            | CliContentBlock::ToolResult { .. }
            | CliContentBlock::Other => None,
        })
}

/// The tool results a `user` line's message hands back: each the id of the
/// call it answers, what the tool printed, and whether it failed.
fn results(message: Option<CliMessage>) -> impl Iterator<Item = (String, Value, bool)> {
    message
        .into_iter()
        .flat_map(|message| message.content)
        .filter_map(|block| match block {
            CliContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some((tool_use_id, content, is_error)),
            CliContentBlock::Text { .. }
            | CliContentBlock::ToolUse { .. }
            | CliContentBlock::Other => None,
        })
}

fn call(id: String, name: String, input: &Value) -> ToolCall {
    let arguments = if input.is_null() {
        NO_ARGUMENTS.to_string()
    } else {
        input.to_string()
    };
    ToolCall::function(id, name, arguments)
}

/// `marker`, followed by claude's own `words` when it gave any.
fn worded(marker: &str, words: &str) -> String {
    let words = words.trim();
    if words.is_empty() {
        marker.to_string()
    } else {
        format!("{marker}: {words}")
    }
}

/// A refused request as a caller reads a rate limit, naming its window and
/// its status.
fn throttled(report: &RateLimitReport) -> String {
    let window = report.kind.as_deref().unwrap_or(UNNAMED_WINDOW);
    let status = report.status.as_deref().unwrap_or_default();
    format!("{THROTTLED} ({window}, {status})")
}

#[cfg(test)]
mod tests {
    use abnegate_llm::Limit;
    use abnegate_llm::Window;
    use abnegate_llm::WindowKind;
    use chrono::DateTime;
    use chrono::Utc;
    use serde_json::Value;
    use serde_json::json;

    use super::THROTTLED;
    use super::Turn;
    use crate::event::AgentEvent;
    use crate::occupancy::Occupancy;
    use crate::parser::claude::CREDITS_REQUIRED;
    use crate::parser::claude::CREDITS_UNCONFIRMED;
    use crate::parser::claude::LONG_CONTEXT_CREDITS_REQUIRED;
    use crate::parser::claude::rate_limit_report::OVERAGE_INCLUDED_WINDOW;
    use crate::stdout_parse_result::StdoutParseResult;

    mod subagents;

    /// Recorded from `claude --verbose --output-format stream-json --print`.
    const SESSION: &str = r#"{"type":"system","subtype":"init","cwd":"/w","session_id":"6f1","tools":["Read","Edit"],"model":"claude-opus-4","permissionMode":"default","apiKeySource":"none"}
{"type":"assistant","message":{"id":"msg_014","type":"message","role":"assistant","model":"claude-opus-4","content":[{"type":"text","text":"Reading the file first."}],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":4,"cache_creation_input_tokens":1200,"cache_read_input_tokens":8400,"output_tokens":7}},"session_id":"6f1"}
{"type":"assistant","message":{"id":"msg_015","type":"message","role":"assistant","model":"claude-opus-4","content":[{"type":"tool_use","id":"toolu_01A","name":"Read","input":{"file_path":"/w/main.rs"}}],"stop_reason":"tool_use","usage":{"input_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":9600,"output_tokens":58}},"session_id":"6f1"}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_01A","content":"fn main() {}"}]},"session_id":"6f1"}
{"type":"assistant","message":{"id":"msg_016","type":"message","role":"assistant","model":"claude-opus-4","content":[{"type":"text","text":"The entry point is empty."}],"stop_reason":"end_turn","usage":{"input_tokens":3,"cache_read_input_tokens":9700,"output_tokens":12}},"session_id":"6f1"}
{"type":"result","subtype":"success","is_error":false,"duration_ms":8421,"duration_api_ms":7980,"num_turns":3,"result":"The entry point is empty.","session_id":"6f1","total_cost_usd":0.0412,"usage":{"input_tokens":9,"cache_creation_input_tokens":1200,"cache_read_input_tokens":27700,"output_tokens":77}}"#;

    /// What claude 2.1.278 streams when the signed-in account cannot fund a
    /// turn on the model it was asked for, put together from its own code.
    const MODEL_REQUIRES_USAGE_CREDITS_STREAM: &str =
        include_str!("../../../tests/fixtures/claude/model-requires-usage-credits.jsonl");

    /// The same, once an allowance that only usage credits extend is spent.
    const OVERAGE_INCLUDED_WINDOW_SPENT_STREAM: &str =
        include_str!("../../../tests/fixtures/claude/overage-included-window-spent.jsonl");

    /// A turn on a long-context model past the context the plan covers, with
    /// usage credits off.
    const LONG_CONTEXT_STREAM: &str =
        include_str!("../../../tests/fixtures/claude/long-context-credits-required.jsonl");

    /// A turn that ran out of turns, which says so only in its `errors`.
    const ERROR_MAX_TURNS_STREAM: &str =
        include_str!("../../../tests/fixtures/claude/error-max-turns-with-errors.jsonl");

    /// A turn whose narration goes before its final answer.
    const NARRATION_THEN_RESULT_STREAM: &str =
        include_str!("../../../tests/fixtures/claude/narration-then-result.jsonl");

    /// A turn on Sonnet that delegates to a subagent on Fable, whose result
    /// names both models' context windows.
    const CONTEXT_WINDOW_STREAM: &str =
        include_str!("../../../tests/fixtures/claude/context-window.jsonl");

    const MAX_TURNS: &str = "Reached maximum number of turns (30)";
    const NARRATION: &str = "#42 returned 500 — LIVE FAIL, retrying.";
    const FINAL_REPORT: &str = "- #42 fix uploads LIVE PASS\nDEPLOY_QA_VERDICT: ALL_VERIFIED";

    const REQUIRES_CREDITS: &str =
        "Fable 5.1 requires usage credits. Switch to another model to continue.";
    const LIMIT_REACHED: &str =
        "You've reached your Fable limit. Switch to another model to continue.";
    const LONG_CONTEXT: &str = "API Error: Usage credits required for 1M context \u{b7} turn on usage credits at claude.ai/settings/usage?from=cc_cli_limit_message (they take effect in a new session)";
    const SESSION_LIMIT: &str = "You've hit your session limit \u{b7} resets 5pm";
    const LIMIT_HIT: &str = "You've hit your limit \u{b7} resets 5pm";
    const OPUS_LIMIT: &str = "You've hit your Opus limit \u{b7} resets Mon 9am";
    const OVERLOADED: &str = "API Error: Repeated 529 Overloaded errors";

    const MODEL_REQUIRES_USAGE_CREDITS: &str = "model_requires_usage_credits";
    const LONG_CONTEXT_CREDITS_REQUIRED_KIND: &str = "long_context_credits_required";

    const AGENT_CALL: &str = "toolu_01Agent";
    const ANSWER: &str =
        "The subagent could not run on this account, so the review is mine: it is sound.";
    const OPUS_ANSWER: &str = "Opus is past its weekly limit, so the review is mine: it is sound.";

    fn interpret_all(sample: &str) -> Vec<AgentEvent> {
        let mut turn = Turn::default();
        let mut events = Vec::new();
        for line in sample.lines() {
            turn.interpret(line, &mut events);
        }
        events
    }

    /// The failures each line of `sample` reports, line by line.
    fn verdicts(sample: &str) -> Vec<Vec<String>> {
        let mut turn = Turn::default();
        sample
            .lines()
            .map(|line| {
                let mut events = Vec::new();
                turn.interpret(line, &mut events);
                failures(&events).into_iter().map(str::to_string).collect()
            })
            .collect()
    }

    fn text(events: &[AgentEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The words of every event that ended the turn unanswered, a limit's
    /// as much as a failure's: a caller reads both the same way.
    fn failures(events: &[AgentEvent]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Failed(message) => Some(message.as_str()),
                AgentEvent::Limited(limit) => Some(limit.message.as_str()),
                _ => None,
            })
            .collect()
    }

    fn limits(events: &[AgentEvent]) -> Vec<&Limit> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Limited(limit) => Some(limit),
                _ => None,
            })
            .collect()
    }

    fn windows(events: &[AgentEvent]) -> Vec<&Window> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Window(window) => Some(window),
                _ => None,
            })
            .collect()
    }

    fn at(seconds: i64) -> Option<DateTime<Utc>> {
        DateTime::from_timestamp(seconds, 0)
    }

    fn diagnostics(events: &[AgentEvent]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Diagnostic(message) => Some(message.as_str()),
                _ => None,
            })
            .collect()
    }

    fn credits(events: &[AgentEvent]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Credits(window) => Some(window.as_str()),
                _ => None,
            })
            .collect()
    }

    fn tools(events: &[AgentEvent]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Tool(call) => Some(call.function.name.as_str()),
                _ => None,
            })
            .collect()
    }

    fn answers(events: &[AgentEvent]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Answer(answer) => Some(answer.as_str()),
                _ => None,
            })
            .collect()
    }

    fn limit(info: &Value) -> String {
        json!({
            "type": "rate_limit_event",
            "rate_limit_info": info,
            "uuid": "3f0e",
            "session_id": "6f1",
        })
        .to_string()
    }

    /// claude's words for a failed request, as the assistant line it writes
    /// for one, typed `kind` when claude gives it a kind.
    fn api_error(words: &str, kind: Option<&str>) -> Value {
        let mut line = json!({
            "type": "assistant",
            "message": {
                "diagnostics": null,
                "model": "<synthetic>",
                "role": "assistant",
                "stop_details": null,
                "stop_reason": "stop_sequence",
                "type": "message",
                "content": [{"type": "text", "text": words}],
            },
            "parent_tool_use_id": null,
            "session_id": "6f1",
            "timestamp": "2026-09-25T03:12:09.412Z",
            "error": "rate_limit",
            "request_id": "req_011CZf",
            "is_api_error_message": true,
        });
        if let Some(kind) = kind {
            line["api_error"] = kind.into();
        }
        line
    }

    fn failed_result(words: &str) -> Value {
        failed_with(words, 429)
    }

    fn failed_with(words: &str, status: u16) -> Value {
        json!({
            "type": "result",
            "subtype": "success",
            "is_error": true,
            "api_error_status": status,
            "result": words,
            "session_id": "6f1",
        })
    }

    fn succeeded(words: &str) -> Value {
        json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": words,
            "session_id": "6f1",
        })
    }

    fn said(words: &str) -> Value {
        json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": words}],
                "usage": {"input_tokens": 12, "cache_read_input_tokens": 3400, "output_tokens": 20},
            },
            "parent_tool_use_id": null,
            "session_id": "6f1",
        })
    }

    fn called(id: &str, name: &str, input: &Value) -> Value {
        json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": id, "name": name, "input": input}],
            },
            "parent_tool_use_id": null,
            "session_id": "6f1",
        })
    }

    /// The main agent's call that starts a subagent on `model`.
    fn delegated(model: &str) -> Value {
        called(
            AGENT_CALL,
            "Agent",
            &json!({
                "description": "Ask for a review",
                "prompt": "Review the change.",
                "subagent_type": "general-purpose",
                "model": model,
            }),
        )
    }

    /// `line` as claude writes it for a subagent: claude runs an Agent in the
    /// background by default and writes each of its lines into the stream,
    /// naming the call that started it, with its API errors and their kinds.
    fn subagents(mut line: Value) -> Value {
        line["parent_tool_use_id"] = AGENT_CALL.into();
        line["subagent_type"] = "general-purpose".into();
        line["task_description"] = "Ask for a review".into();
        line
    }

    fn stream(lines: &[String]) -> String {
        lines.join("\n")
    }

    /// The limit a refused window `info` reads as, in claude's `words`.
    fn throttled(info: &Value, words: &str) -> String {
        let window = info["rateLimitType"].as_str().unwrap_or("request");
        let status = info["status"].as_str().unwrap_or_default();
        format!("{THROTTLED} ({window}, {status}): {words}")
    }

    fn credits_required(words: &str) -> String {
        format!("{CREDITS_REQUIRED}: {words}")
    }

    /// The event claude writes when it refuses a request past the plan's
    /// weekly Opus limit, which names no agent.
    fn refused_past_the_opus_limit() -> String {
        limit(&json!({
            "status": "rejected",
            "resetsAt": 1_790_208_000,
            "rateLimitType": "seven_day_opus",
            "isUsingOverage": false,
        }))
    }

    fn refused_for_credits() -> String {
        limit(&json!({
            "status": "rejected",
            "overageStatus": "rejected",
            "overageDisabledReason": "overage_not_provisioned",
            "isUsingOverage": false,
            "errorCode": "credits_required",
        }))
    }

    #[test]
    fn a_refused_request_reads_as_a_rate_limit_to_the_caller_naming_its_window() {
        for window in [
            "five_hour",
            "seven_day",
            "seven_day_opus",
            "seven_day_sonnet",
            "overage",
        ] {
            for info in [
                json!({"status": "rejected", "rateLimitType": window}),
                json!({
                    "status": "rejected",
                    "rateLimitType": window,
                    "overageStatus": "rejected",
                    "overageDisabledReason": "out_of_credits",
                    "isUsingOverage": false,
                }),
            ] {
                let events = interpret_all(&stream(&[
                    limit(&info),
                    api_error(LIMIT_HIT, None).to_string(),
                ]));

                let failures = failures(&events);
                assert_eq!(failures, [throttled(&info, LIMIT_HIT)]);
                assert!(failures[0].contains(window), "{failures:?}");
                assert!(failures[0].contains("rejected"), "{failures:?}");
            }
        }
    }

    #[test]
    fn a_refused_request_that_names_no_window_is_a_rate_limit_on_the_request() {
        let info = json!({"status": "rejected"});
        let events = interpret_all(&stream(&[
            limit(&info),
            api_error(LIMIT_HIT, None).to_string(),
        ]));

        assert_eq!(failures(&events), [throttled(&info, LIMIT_HIT)]);
    }

    /// claude reports a plan's window as rejected once the account's usage
    /// credits carry a request past it, and the request goes through.
    #[test]
    fn a_request_usage_credits_carry_past_the_plans_window_is_not_a_failure() {
        for window in [
            "five_hour",
            "seven_day",
            "seven_day_opus",
            "seven_day_sonnet",
            "seven_day_overage_included",
        ] {
            for overage in ["allowed", "allowed_warning"] {
                let line = limit(&json!({
                    "status": "rejected",
                    "resetsAt": 1_790_208_000,
                    "rateLimitType": window,
                    "overageStatus": overage,
                    "isUsingOverage": true,
                }));
                let events = interpret_all(&line);
                assert!(
                    failures(&events).is_empty() && diagnostics(&events).is_empty(),
                    "{window} on usage credits ({overage}) was treated as a refusal: {events:?}"
                );
            }
        }
    }

    /// claude runs a request on usage credits only when it says so. Credits
    /// the account allows beside any other status, or beside none, carried
    /// nothing past a window.
    #[test]
    fn allowed_usage_credits_are_headroom_only_when_claude_says_they_are_in_use() {
        for info in [
            json!({
                "status": "rejected",
                "rateLimitType": "five_hour",
                "overageStatus": "allowed",
                "isUsingOverage": false,
            }),
            json!({"status": "rejected", "rateLimitType": "five_hour", "overageStatus": "allowed"}),
            json!({"status": "exhausted", "rateLimitType": "five_hour", "overageStatus": "allowed"}),
            json!({"rateLimitType": "five_hour", "overageStatus": "allowed_warning"}),
        ] {
            let events = interpret_all(&stream(&[
                limit(&info),
                api_error(SESSION_LIMIT, None).to_string(),
            ]));

            assert!(
                matches!(failures(&events).as_slice(), [message] if message.starts_with(THROTTLED)),
                "{info}: {events:?}"
            );
            assert!(credits(&events).is_empty(), "{info}: {events:?}");
        }
    }

    /// A request past the plan's window that failed anyway still ends the
    /// turn a failure, and claude's words for it are no answer.
    #[test]
    fn a_failed_request_beside_allowed_usage_credits_still_fails_the_turn() {
        let info = json!({
            "status": "rejected",
            "resetsAt": 1_790_208_000,
            "rateLimitType": "five_hour",
            "overageStatus": "allowed",
            "isUsingOverage": false,
        });
        let events = interpret_all(&stream(&[
            limit(&info),
            api_error(SESSION_LIMIT, None).to_string(),
            failed_result(SESSION_LIMIT).to_string(),
        ]));

        assert_eq!(
            failures(&events).first().copied(),
            Some(throttled(&info, SESSION_LIMIT).as_str()),
            "{events:?}"
        );
        assert_eq!(text(&events), "", "claude's failure read as an answer");
    }

    /// Once usage credits carry a turn, a request that fails anyway ends in
    /// claude's API error, which the failed result then reports.
    #[test]
    fn an_api_error_never_streams_into_the_answer() {
        const OUTPUT_CAP: &str =
            "API Error: Claude's response exceeded the 32000 output token maximum.";
        for (line, words) in [
            (api_error(SESSION_LIMIT, None), SESSION_LIMIT),
            (api_error(OUTPUT_CAP, Some("max_output_tokens")), OUTPUT_CAP),
        ] {
            let events = interpret_all(&stream(&[
                limit(&json!({
                    "status": "rejected",
                    "rateLimitType": "five_hour",
                    "overageStatus": "allowed",
                    "isUsingOverage": true,
                })),
                line.to_string(),
                failed_result(words).to_string(),
            ]));

            assert_eq!(text(&events), "", "claude's API error read as an answer");
            assert_eq!(failures(&events), [words]);
        }
    }

    #[test]
    fn a_turn_on_a_model_the_account_cannot_fund_fails_in_claudes_words_and_not_as_a_rate_limit() {
        for (stream, words) in [
            (MODEL_REQUIRES_USAGE_CREDITS_STREAM, REQUIRES_CREDITS),
            (OVERAGE_INCLUDED_WINDOW_SPENT_STREAM, LIMIT_REACHED),
        ] {
            let events = interpret_all(stream);

            let failures = failures(&events);
            assert!(!failures.is_empty(), "no failure in {events:?}");
            assert!(
                failures
                    .iter()
                    .all(|message| *message == credits_required(words)),
                "{failures:?}"
            );
            assert!(
                failures.iter().all(|message| !message.contains(THROTTLED)),
                "{failures:?}"
            );
            assert_eq!(
                limits(&events).len(),
                failures.len(),
                "a refusal was not a limit: {events:?}"
            );
            assert!(
                limits(&events).iter().all(|limit| limit.credits),
                "{events:?}"
            );
            assert_eq!(text(&events), "", "claude's refusal read as an answer");
        }
    }

    /// claude writes the refused request's rate_limit_event, then the line
    /// that types the refusal. The event waits for that line, and the
    /// failed result that follows reports the same refusal.
    #[test]
    fn the_event_before_a_credits_refusal_waits_for_the_line_that_names_it() {
        for (stream, words) in [
            (MODEL_REQUIRES_USAGE_CREDITS_STREAM, REQUIRES_CREDITS),
            (OVERAGE_INCLUDED_WINDOW_SPENT_STREAM, LIMIT_REACHED),
        ] {
            assert_eq!(
                verdicts(stream),
                [
                    Vec::new(),
                    Vec::new(),
                    vec![credits_required(words)],
                    vec![credits_required(words)]
                ],
                "{stream}"
            );
        }
    }

    /// Each reason claude refuses a turn on usage credits for has its own
    /// words, which name what the account's owner can do.
    #[test]
    fn every_credits_refusal_reaches_the_caller_in_claudes_own_words() {
        for (reason, words) in [
            ("overage_not_provisioned", REQUIRES_CREDITS),
            (
                "out_of_credits",
                "You're out of usage credits. Switch to another model to continue.",
            ),
            (
                "org_level_disabled_until",
                "You've hit your monthly spend limit. Switch to another model to continue.",
            ),
            (
                "member_level_disabled",
                "Fable 5.1 requires usage credits. Switch to another model, or manage usage credits at claude.ai/admin-settings/usage, to continue.",
            ),
        ] {
            let events = interpret_all(&stream(&[
                limit(&json!({
                    "status": "rejected",
                    "overageStatus": "rejected",
                    "overageDisabledReason": reason,
                    "isUsingOverage": false,
                    "errorCode": "credits_required",
                })),
                api_error(words, Some(MODEL_REQUIRES_USAGE_CREDITS)).to_string(),
                failed_result(words).to_string(),
            ]));

            assert_eq!(
                failures(&events).first().copied(),
                Some(credits_required(words).as_str()),
                "{reason}"
            );
            assert!(
                limits(&events).first().is_some_and(|limit| limit.credits),
                "{reason}: {events:?}"
            );
            assert_eq!(text(&events), "", "{reason}");
        }
    }

    /// claude holds nothing against the account when it could not look its
    /// usage credits up, so neither does the caller.
    #[test]
    fn a_refusal_claude_could_not_check_the_usage_credits_of_is_not_a_credits_refusal() {
        for reason in ["fetch_error", "unknown"] {
            for (kind, words) in [
                (MODEL_REQUIRES_USAGE_CREDITS, REQUIRES_CREDITS),
                (MODEL_REQUIRES_USAGE_CREDITS, LIMIT_REACHED),
                (LONG_CONTEXT_CREDITS_REQUIRED_KIND, LONG_CONTEXT),
            ] {
                let events = interpret_all(&stream(&[
                    limit(&json!({
                        "status": "rejected",
                        "overageStatus": "rejected",
                        "overageDisabledReason": reason,
                        "isUsingOverage": false,
                        "errorCode": "credits_required",
                    })),
                    api_error(words, Some(kind)).to_string(),
                    failed_result(words).to_string(),
                ]));

                let failures = failures(&events);
                assert_eq!(
                    failures.first().copied(),
                    Some(format!("{CREDITS_UNCONFIRMED} ({reason}): {words}").as_str()),
                    "{reason}, {kind}"
                );
                assert!(
                    failures
                        .iter()
                        .all(|message| !message.contains(CREDITS_REQUIRED)
                            && !message.contains(LONG_CONTEXT_CREDITS_REQUIRED)),
                    "{failures:?}"
                );
                assert!(
                    matches!(
                        events.iter().find(|event| event.terminal()),
                        Some(AgentEvent::Failed(_))
                    ),
                    "{reason}, {kind}: claude could not check the credits, so no limit: {events:?}"
                );
                assert!(
                    limits(&events).iter().all(|limit| !limit.credits),
                    "{reason}, {kind}: {events:?}"
                );
            }
        }
    }

    /// A report that refuses nothing in words of its own, such as a credits
    /// code, leaves the refused window's limit as it was: its words, its
    /// window and its reset time all come from the one refusal.
    #[test]
    fn a_limit_keeps_the_window_and_reset_of_the_refusal_it_words() {
        let events = interpret_all(&stream(&[
            refused_past_the_opus_limit(),
            refused_for_credits(),
            api_error("You've hit your limit", None).to_string(),
        ]));

        let limits = limits(&events);
        assert_eq!(limits.len(), 1, "{events:?}");
        assert!(limits[0].message.contains("seven_day_opus"), "{events:?}");
        assert_eq!(
            limits[0].window.as_ref().map(|window| window.name.as_str()),
            Some("seven_day_opus"),
            "{events:?}"
        );
        assert_eq!(limits[0].resets_at, at(1_790_208_000), "{events:?}");
    }

    /// The reason that counts is the one claude gave for the request it
    /// refused, the latest before the refusal.
    #[test]
    fn the_latest_event_decides_whether_claude_could_check_the_usage_credits() {
        let unchecked = limit(&json!({
            "status": "allowed",
            "overageStatus": "rejected",
            "overageDisabledReason": "fetch_error",
            "isUsingOverage": false,
        }));
        let refused = refused_for_credits();
        let refusal = api_error(REQUIRES_CREDITS, Some(MODEL_REQUIRES_USAGE_CREDITS)).to_string();

        let checked = interpret_all(&stream(&[
            unchecked.clone(),
            refused.clone(),
            refusal.clone(),
        ]));
        assert_eq!(failures(&checked), [credits_required(REQUIRES_CREDITS)]);

        let unchecked_last = interpret_all(&stream(&[refused, unchecked, refusal]));
        assert_eq!(
            failures(&unchecked_last),
            [format!(
                "{CREDITS_UNCONFIRMED} (fetch_error): {REQUIRES_CREDITS}"
            )]
        );
    }

    /// claude's typed kind names a credits refusal. The server's code does
    /// not: it is the same for any request usage credits could have carried.
    #[test]
    fn the_servers_credits_code_alone_never_names_a_refusal_kind() {
        let words = "Usage credits are required for this request.";
        let mut refusal = api_error(words, None);
        refusal["api_error_code"] = "credits_required".into();
        let mut result = failed_result(words);
        result["api_error_code"] = "credits_required".into();

        let events = interpret_all(&stream(&[
            refused_for_credits(),
            refusal.to_string(),
            result.to_string(),
        ]));

        assert_eq!(failures(&events), [words]);
    }

    #[test]
    fn a_long_context_refusal_never_reads_as_a_model_refusal() {
        let events = interpret_all(LONG_CONTEXT_STREAM);

        let failures = failures(&events);
        assert_eq!(
            failures.first().copied(),
            Some(format!("{LONG_CONTEXT_CREDITS_REQUIRED}: {LONG_CONTEXT}").as_str()),
            "{events:?}"
        );
        assert!(
            failures
                .iter()
                .all(|message| !message.contains(CREDITS_REQUIRED)),
            "{failures:?}"
        );
        assert_eq!(text(&events), "", "claude's refusal read as an answer");
    }

    /// claude hands a subagent's refusal to the main agent as the result of
    /// the call that started it, and the main agent answers without it.
    #[test]
    fn a_subagents_refusal_leaves_the_main_agent_to_answer() {
        for (kind, words) in [
            (MODEL_REQUIRES_USAGE_CREDITS, REQUIRES_CREDITS),
            (LONG_CONTEXT_CREDITS_REQUIRED_KIND, LONG_CONTEXT),
        ] {
            let events = interpret_all(&stream(&[
                delegated("fable").to_string(),
                refused_for_credits(),
                subagents(api_error(words, Some(kind))).to_string(),
                said(ANSWER).to_string(),
                succeeded(ANSWER).to_string(),
            ]));

            assert!(failures(&events).is_empty(), "{kind}: {events:?}");
            assert_eq!(text(&events), ANSWER, "{kind}");
            assert!(
                matches!(events.last(), Some(AgentEvent::Finished { .. })),
                "{kind}: {events:?}"
            );
        }
    }

    /// A main agent on Sonnet starts a subagent on Opus past the plan's
    /// weekly Opus limit. claude refuses the subagent's request on an event
    /// that names no agent, hands the refusal to the main agent as the
    /// result of the call that started it, and the main agent answers.
    #[test]
    fn a_subagents_refused_window_leaves_the_main_agent_to_answer() {
        let events = interpret_all(&stream(&[
            delegated("opus").to_string(),
            refused_past_the_opus_limit(),
            subagents(api_error(OPUS_LIMIT, None)).to_string(),
            said(OPUS_ANSWER).to_string(),
            succeeded(OPUS_ANSWER).to_string(),
        ]));

        assert!(failures(&events).is_empty(), "{events:?}");
        assert_eq!(text(&events), OPUS_ANSWER);
        assert!(
            matches!(events.last(), Some(AgentEvent::Finished { .. })),
            "{events:?}"
        );
    }

    /// A line the main agent writes after a refused window shows that its
    /// own request went through, so the refusal was a subagent's, and a
    /// later failure of the main agent's is judged by its own words.
    #[test]
    fn a_refused_window_the_main_agent_writes_past_was_not_its_own() {
        let events = interpret_all(&stream(&[
            delegated("opus").to_string(),
            refused_past_the_opus_limit(),
            subagents(api_error(OPUS_LIMIT, None)).to_string(),
            said("Opus is past its weekly limit, so I will review the change.").to_string(),
            api_error(OVERLOADED, None).to_string(),
            failed_with(OVERLOADED, 529).to_string(),
        ]));

        assert!(
            matches!(events.last(), Some(AgentEvent::Failed(message)) if message == OVERLOADED),
            "{events:?}"
        );
    }

    /// claude refuses a request past the plan's window on an event that
    /// names no agent, then writes the refusal as the line of the agent that
    /// sent it. The main agent's refusal fails the turn there, in claude's
    /// words after the window, and the failed result says the same.
    #[test]
    fn the_main_agents_refused_window_still_fails_the_turn_as_a_rate_limit() {
        let info = json!({
            "status": "rejected",
            "resetsAt": 1_790_208_000,
            "rateLimitType": "five_hour",
            "isUsingOverage": false,
        });
        let refused = throttled(&info, SESSION_LIMIT);

        assert_eq!(
            verdicts(&stream(&[
                limit(&info),
                api_error(SESSION_LIMIT, None).to_string(),
                failed_result(SESSION_LIMIT).to_string(),
            ])),
            [Vec::new(), vec![refused.clone()], vec![refused]]
        );
    }

    /// A subagent's words are its own conversation's, and what it reaches
    /// for is a step of its run, not one of the main agent's calls.
    #[test]
    fn a_subagents_words_never_reach_the_answer() {
        let events = interpret_all(&stream(&[
            delegated("fable").to_string(),
            subagents(said("Reading the diff first.")).to_string(),
            subagents(called(
                "toolu_02Read",
                "Read",
                &json!({"file_path": "/w/main.rs"}),
            ))
            .to_string(),
            subagents(said("The change is sound.")).to_string(),
            said(ANSWER).to_string(),
            succeeded(ANSWER).to_string(),
        ]));

        assert_eq!(text(&events), ANSWER);
        assert_eq!(tools(&events), ["Agent"]);
    }

    #[test]
    fn a_subagents_token_counts_are_not_the_turns() {
        let mut subagent = subagents(said("The change is sound."));
        subagent["message"]["usage"] = json!({"input_tokens": 190_000, "output_tokens": 900});

        let events = interpret_all(&stream(&[
            delegated("fable").to_string(),
            subagent.to_string(),
            said(ANSWER).to_string(),
        ]));

        let prompts: Vec<u32> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Usage(usage) => Some(usage.prompt_tokens),
                _ => None,
            })
            .collect();
        assert_eq!(prompts, [12 + 3400]);
    }

    #[test]
    fn the_main_agents_refusal_still_fails_the_turn_beside_a_subagent() {
        for (kind, words, marker) in [
            (
                MODEL_REQUIRES_USAGE_CREDITS,
                REQUIRES_CREDITS,
                CREDITS_REQUIRED,
            ),
            (
                LONG_CONTEXT_CREDITS_REQUIRED_KIND,
                LONG_CONTEXT,
                LONG_CONTEXT_CREDITS_REQUIRED,
            ),
        ] {
            let events = interpret_all(&stream(&[
                delegated("fable").to_string(),
                subagents(called(
                    "toolu_02Read",
                    "Read",
                    &json!({"file_path": "/w/main.rs"}),
                ))
                .to_string(),
                refused_for_credits(),
                api_error(words, Some(kind)).to_string(),
                failed_result(words).to_string(),
            ]));

            assert_eq!(
                failures(&events).first().copied(),
                Some(format!("{marker}: {words}").as_str()),
                "{kind}: {events:?}"
            );
            assert_eq!(text(&events), "", "{kind}");
        }
    }

    /// A turn usage credits carried past the plan's window says so once, with
    /// the window, however many events repeat it.
    #[test]
    fn a_turn_on_usage_credits_reports_the_window_they_carried_it_past_once() {
        let answer = SESSION.lines().nth(4).expect("the session's answer");
        let finished = SESSION.lines().last().expect("the session's result");
        let events = interpret_all(&stream(&[
            limit(
                &json!({"status": "allowed", "rateLimitType": "five_hour", "isUsingOverage": false}),
            ),
            limit(&json!({
                "status": "rejected",
                "rateLimitType": "five_hour",
                "overageStatus": "allowed",
                "isUsingOverage": true,
            })),
            answer.to_string(),
            limit(&json!({
                "status": "rejected",
                "rateLimitType": "seven_day",
                "overageStatus": "allowed_warning",
                "isUsingOverage": true,
            })),
            finished.to_string(),
        ]));

        assert_eq!(credits(&events), ["five_hour"]);
        assert!(failures(&events).is_empty(), "{events:?}");
        assert!(diagnostics(&events).is_empty(), "{events:?}");
    }

    #[test]
    fn a_turn_inside_the_plans_window_reports_no_usage_credits() {
        let headroom = limit(&json!({
            "status": "allowed_warning",
            "rateLimitType": "five_hour",
            "overageStatus": "allowed",
            "isUsingOverage": false,
        }));
        let events = interpret_all(&format!("{headroom}\n{SESSION}"));

        assert!(credits(&events).is_empty(), "{events:?}");
    }

    /// Nothing here names a credits refusal by the words it is written in: a
    /// failed result keeps them, whatever they resemble.
    #[test]
    fn a_failed_result_that_resembles_a_credits_refusal_keeps_its_own_words() {
        for words in [
            REQUIRES_CREDITS,
            LIMIT_REACHED,
            "Opus 5 requires usage credits. Switch to another model to continue.",
            "Usage credits are required for this model.",
            "You have hit your weekly limit",
        ] {
            let events = interpret_all(&failed_result(words).to_string());

            assert_eq!(failures(&events), [words], "{words}");
            assert!(
                limits(&events).iter().all(|limit| !limit.credits),
                "{words}: {events:?}"
            );
        }
    }

    /// A refused window is remembered as a diagnostic, so a stream that dies
    /// before the line that answers it still fails the run, as a rate limit.
    #[test]
    fn a_stream_that_ends_on_a_refused_window_still_fails_as_a_rate_limit() {
        let info = json!({"status": "rejected", "rateLimitType": "five_hour"});
        let events = interpret_all(&limit(&info));
        assert!(failures(&events).is_empty(), "{events:?}");

        let mut result: StdoutParseResult = events.into_iter().collect();
        result.conclude();

        let failure = result.failure.expect("a failed run");
        assert!(failure.starts_with(THROTTLED), "{failure}");
        assert!(failure.contains("five_hour"), "{failure}");
    }

    #[test]
    fn a_headroom_report_becomes_a_window_and_not_a_failure() {
        for status in ["allowed", "allowed_warning"] {
            let events = interpret_all(&limit(&json!({
                "status": status,
                "resetsAt": 1_790_208_000,
                "rateLimitType": "five_hour",
                "utilization": 0.43,
                "isUsingOverage": false,
            })));

            assert!(failures(&events).is_empty(), "{status}: {events:?}");
            assert!(diagnostics(&events).is_empty(), "{status}: {events:?}");
            let expected = Window::new("five_hour")
                .with_used_percent(43.0)
                .with_resets_at(at(1_790_208_000).expect("a valid timestamp"))
                .with_kind(WindowKind::FiveHours);
            assert_eq!(windows(&events), [&expected], "{status}");
            assert_eq!(
                windows(&events)[0].kind,
                Some(WindowKind::FiveHours),
                "{status}"
            );
        }
    }

    #[test]
    fn a_headroom_report_without_its_use_or_its_window_reports_no_window() {
        for info in [
            json!({"status": "allowed", "rateLimitType": "five_hour", "isUsingOverage": false}),
            json!({"status": "allowed", "utilization": 0.43, "isUsingOverage": false}),
        ] {
            let events = interpret_all(&limit(&info));
            assert!(events.is_empty(), "{info}: {events:?}");
        }
    }

    /// The window keeps the name claude streams it under, so a caller that
    /// records windows under names of its own maps each by its kind.
    #[test]
    fn a_window_keeps_the_name_claude_streams_it_under_beside_its_kind() {
        for (streamed, kind) in [
            ("five_hour", Some(WindowKind::FiveHours)),
            ("seven_day", Some(WindowKind::SevenDays)),
            ("seven_day_opus", Some(WindowKind::SevenDaysOpus)),
            ("seven_day_sonnet", Some(WindowKind::SevenDaysSonnet)),
            ("hourly_burst", None),
        ] {
            let reported = interpret_all(&limit(&json!({
                "status": "allowed",
                "rateLimitType": streamed,
                "utilization": 0.43,
                "isUsingOverage": false,
            })));
            let refused = interpret_all(&stream(&[
                limit(&json!({"status": "rejected", "rateLimitType": streamed})),
                api_error(SESSION_LIMIT, None).to_string(),
            ]));

            let [window] = windows(&reported)[..] else {
                panic!("{streamed}: expected one window, got {reported:?}");
            };
            assert_eq!((window.name.as_str(), window.kind), (streamed, kind));
            let [limit] = limits(&refused)[..] else {
                panic!("{streamed}: expected one limit, got {refused:?}");
            };
            let window = limit.window.as_ref().expect("the refused window");
            assert_eq!((window.name.as_str(), window.kind), (streamed, kind));
            assert_eq!(
                limit.message,
                format!("{THROTTLED} ({streamed}, rejected): {SESSION_LIMIT}"),
                "a caller still reads the window as claude names it"
            );
        }
    }

    #[test]
    fn a_report_on_the_credits_window_is_no_window_of_the_plan() {
        let events = interpret_all(&limit(&json!({
            "status": "allowed",
            "rateLimitType": OVERAGE_INCLUDED_WINDOW,
            "utilization": 1.0,
            "isUsingOverage": false,
        })));

        assert!(windows(&events).is_empty(), "{events:?}");
    }

    #[test]
    fn a_refused_request_past_the_credits_window_is_a_limit_with_its_reset_time_and_no_window() {
        let events = interpret_all(OVERAGE_INCLUDED_WINDOW_SPENT_STREAM);

        let limits = limits(&events);
        assert!(!limits.is_empty(), "no limit in {events:?}");
        for limit in limits {
            assert!(limit.credits, "{limit:?}");
            assert_eq!(limit.resets_at, at(1_790_208_000), "{limit:?}");
            assert_eq!(
                limit.window, None,
                "the window usage credits carry a turn past is no part of the plan's usage"
            );
        }
    }

    #[test]
    fn a_rejected_window_keeps_the_wording_a_caller_reads() {
        let events = interpret_all(&stream(&[
            limit(&json!({
                "status": "rejected",
                "resetsAt": 1_790_208_000,
                "rateLimitType": "five_hour",
                "utilization": 1.0,
                "isUsingOverage": false,
            })),
            api_error(SESSION_LIMIT, None).to_string(),
        ]));

        let [AgentEvent::Diagnostic(_), AgentEvent::Limited(limit)] = events.as_slice() else {
            panic!("expected the remembered window, then one limit, got {events:?}");
        };
        assert_eq!(
            limit.message,
            format!("rate limit reached (five_hour, rejected): {SESSION_LIMIT}")
        );
        assert!(!limit.credits, "{limit:?}");
        assert_eq!(limit.resets_at, at(1_790_208_000));
        assert_eq!(
            limit.window.as_ref().and_then(|window| window.used_percent),
            Some(100.0)
        );
        assert_eq!(
            limit.window.as_ref().and_then(|window| window.kind),
            Some(WindowKind::FiveHours)
        );
    }

    #[test]
    fn a_refused_window_that_does_not_say_its_use_is_counted_as_spent() {
        let events = interpret_all(&stream(&[
            limit(&json!({"status": "rejected", "rateLimitType": "seven_day"})),
            api_error(LIMIT_HIT, None).to_string(),
        ]));

        let [limit] = limits(&events)[..] else {
            panic!("expected one limit, got {events:?}");
        };
        assert_eq!(
            limit.window.as_ref().and_then(|window| window.used_percent),
            Some(100.0)
        );
    }

    #[test]
    fn a_failed_result_in_limit_words_or_a_429_is_a_limit_without_a_reset_time() {
        for line in [
            failed_with(SESSION_LIMIT, 400),
            failed_with("Request refused", 429),
            json!({"type": "result", "subtype": "error_during_execution", "is_error": true, "result": LIMIT_HIT}),
        ] {
            let events = interpret_all(&line.to_string());

            let [AgentEvent::Limited(limit)] = events.as_slice() else {
                panic!("expected one limit for {line}, got {events:?}");
            };
            assert_eq!(Some(limit.message.as_str()), line["result"].as_str());
            assert_eq!(limit.resets_at, None, "{line}");
            assert_eq!(limit.window, None, "{line}");
            assert!(!limit.credits, "{line}");
        }
    }

    #[test]
    fn a_signed_out_result_is_a_failure_and_not_a_limit() {
        for line in [
            failed_with("Not logged in \u{b7} Please run /login", 401),
            failed_with("Invalid API key \u{b7} Please run /login", 401),
            failed_with("You've hit your session limit, so sign in again", 401),
            json!({"type": "result", "subtype": "error_during_execution", "is_error": true, "result": "OAuth token has expired. Please obtain a new token or refresh your existing token."}),
        ] {
            let events = interpret_all(&line.to_string());

            assert!(
                matches!(events.as_slice(), [AgentEvent::Failed(message)] if Some(message.as_str()) == line["result"].as_str()),
                "{line}: {events:?}"
            );
        }
    }

    #[test]
    fn a_signed_out_result_after_a_refused_window_is_still_a_failure() {
        let events = interpret_all(&stream(&[
            limit(&json!({
                "status": "rejected",
                "resetsAt": 1_790_208_000,
                "rateLimitType": "five_hour",
                "isUsingOverage": false,
            })),
            failed_with("Not logged in \u{b7} Please run /login", 401).to_string(),
        ]));

        assert!(
            matches!(
                events.last(),
                Some(AgentEvent::Failed(message)) if message == "Not logged in \u{b7} Please run /login"
            ),
            "{events:?}"
        );
        assert!(limits(&events).is_empty(), "{events:?}");
    }

    #[test]
    fn a_reset_time_reads_as_seconds_or_a_timestamp() {
        for (resets_at, expected) in [
            (json!(1_790_208_000), at(1_790_208_000)),
            (json!("1790208000"), at(1_790_208_000)),
            (json!("2026-09-24T04:00:00Z"), at(1_790_222_400)),
            (json!("2026-09-24T16:00:00+12:00"), at(1_790_222_400)),
            (json!("next Tuesday"), None),
            (json!({"seconds": 1_790_208_000}), None),
            (Value::Null, None),
        ] {
            let events = interpret_all(&stream(&[
                limit(&json!({
                    "status": "rejected",
                    "resetsAt": resets_at,
                    "rateLimitType": "five_hour",
                    "isUsingOverage": false,
                })),
                api_error(SESSION_LIMIT, None).to_string(),
            ]));

            let [limit] = limits(&events)[..] else {
                panic!("{resets_at}: a reset time it cannot read lost the refusal: {events:?}");
            };
            assert_eq!(limit.resets_at, expected, "{resets_at}");
        }
    }

    #[test]
    fn a_reset_time_at_the_top_of_the_event_still_reaches_the_limit() {
        let line = json!({
            "type": "rate_limit_event",
            "rate_limit_info": {"status": "rejected", "rateLimitType": "five_hour"},
            "resetsAt": "2026-09-24T04:00:00Z",
        });
        let events = interpret_all(&stream(&[
            line.to_string(),
            api_error(SESSION_LIMIT, None).to_string(),
        ]));

        let [limit] = limits(&events)[..] else {
            panic!("expected one limit, got {events:?}");
        };
        assert_eq!(limit.resets_at, at(1_790_222_400));
    }

    #[test]
    fn a_use_of_another_shape_loses_neither_the_refusal_nor_its_window() {
        let events = interpret_all(&stream(&[
            limit(&json!({
                "status": "rejected",
                "rateLimitType": "five_hour",
                "utilization": "most of it",
            })),
            api_error(SESSION_LIMIT, None).to_string(),
        ]));

        let [limit] = limits(&events)[..] else {
            panic!("a use it cannot read lost the refusal: {events:?}");
        };
        assert_eq!(
            limit.window.as_ref().and_then(|window| window.used_percent),
            Some(100.0)
        );
    }

    #[test]
    fn a_window_the_main_agent_writes_past_leaves_no_reset_time_on_a_later_limit() {
        let events = interpret_all(&stream(&[
            refused_past_the_opus_limit(),
            said("Opus is past its weekly limit, so I will answer myself.").to_string(),
            failed_with(LIMIT_HIT, 429).to_string(),
        ]));

        let [limit] = limits(&events)[..] else {
            panic!("expected one limit, got {events:?}");
        };
        assert_eq!(limit.message, LIMIT_HIT);
        assert_eq!(limit.resets_at, None, "{limit:?}");
        assert_eq!(limit.window, None, "{limit:?}");
    }

    #[test]
    fn a_turn_out_of_turns_fails_in_its_joined_errors_and_not_its_subtype() {
        let events = interpret_all(ERROR_MAX_TURNS_STREAM);

        assert_eq!(failures(&events), [MAX_TURNS]);
        assert!(
            matches!(events.last(), Some(AgentEvent::Failed(message)) if message == MAX_TURNS),
            "{events:?}"
        );
        assert!(answers(&events).is_empty(), "{events:?}");
        assert_eq!(text(&events), "Let me check.");
    }

    #[test]
    fn a_failed_results_words_and_errors_are_joined_a_line_apiece() {
        for (line, expected) in [
            (
                json!({
                    "type": "result",
                    "subtype": "error_max_turns",
                    "is_error": true,
                    "result": " \n ",
                    "errors": [MAX_TURNS, {"code": 7}, null, "  "],
                }),
                format!("{MAX_TURNS}\n{{\"code\":7}}"),
            ),
            (
                json!({
                    "type": "result",
                    "subtype": "error_during_execution",
                    "is_error": true,
                    "result": "The run failed.",
                    "errors": ["  Tool permission denied: Bash  "],
                }),
                "The run failed.\nTool permission denied: Bash".to_string(),
            ),
            (
                json!({
                    "type": "result",
                    "subtype": "error_max_turns",
                    "is_error": true,
                    "errors": [],
                }),
                "error_max_turns".to_string(),
            ),
        ] {
            let events = interpret_all(&line.to_string());

            assert!(
                matches!(events.as_slice(), [AgentEvent::Failed(message)] if *message == expected),
                "{line}: {events:?}"
            );
        }
    }

    #[test]
    fn errors_in_a_limits_words_are_a_limit() {
        let line = json!({
            "type": "result",
            "subtype": "error_during_execution",
            "is_error": true,
            "errors": [LIMIT_HIT],
        });
        let events = interpret_all(&line.to_string());

        assert!(
            matches!(events.as_slice(), [AgentEvent::Limited(limit)] if limit.message == LIMIT_HIT),
            "{events:?}"
        );
    }

    #[test]
    fn narration_followed_by_a_final_result_answers_in_the_result_alone() {
        let events = interpret_all(NARRATION_THEN_RESULT_STREAM);

        assert_eq!(answers(&events), [FINAL_REPORT]);
        assert_eq!(text(&events), format!("{NARRATION}{FINAL_REPORT}"));
        assert!(
            matches!(
                events.as_slice(),
                [.., AgentEvent::Answer(_), AgentEvent::Finished { finish_reason }]
                    if finish_reason.as_deref() == Some("success")
            ),
            "the answer comes just before the finish: {events:?}"
        );

        let result: StdoutParseResult = events.into_iter().collect();
        assert_eq!(result.answer.as_deref(), Some(FINAL_REPORT));
        assert_eq!(result.reply(), FINAL_REPORT);
        assert!(result.finished);
        assert!(result.failure.is_none());
    }

    #[test]
    fn a_failed_or_blank_result_is_no_answer() {
        for line in [
            failed_result(SESSION_LIMIT),
            failed_with("Invalid API key provided", 400),
            json!({"type": "result", "subtype": "error_during_execution", "is_error": true, "result": "It broke."}),
            succeeded(" \n "),
            json!({"type": "result", "subtype": "success", "is_error": false}),
        ] {
            let events = interpret_all(&line.to_string());

            assert!(answers(&events).is_empty(), "{line}: {events:?}");
            assert!(
                events.last().is_some_and(AgentEvent::terminal),
                "{line}: {events:?}"
            );
        }
    }

    fn occupancies(events: &[AgentEvent]) -> Vec<Occupancy> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Occupancy(occupancy) => Some(*occupancy),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_main_agents_model_names_the_window_and_its_latest_request_the_fill() {
        let events = interpret_all(CONTEXT_WINDOW_STREAM);

        assert_eq!(
            occupancies(&events),
            [Occupancy::reported(200_000, Some(3 + 120 + 16_500 + 9)).expect("a window")]
        );
        let result: StdoutParseResult = events.into_iter().collect();
        assert_eq!(result.occupancy, Occupancy::reported(200_000, Some(16_632)));
        assert!(result.finished);
    }

    #[test]
    fn without_the_main_agents_lines_the_widest_window_is_reported_unfilled() {
        let subagent_only: String = CONTEXT_WINDOW_STREAM
            .lines()
            .filter(|line| {
                !(line.contains(r#""type":"assistant""#)
                    && line.contains(r#""parent_tool_use_id":null"#))
            })
            .map(|line| format!("{line}\n"))
            .collect();

        let events = interpret_all(&subagent_only);

        assert_eq!(
            occupancies(&events),
            [Occupancy::reported(1_000_000, None).expect("a window")]
        );
        assert!(
            events.last().is_some_and(AgentEvent::terminal),
            "{events:?}"
        );
    }

    #[test]
    fn a_main_model_the_result_does_not_name_reports_the_widest_window_unfilled() {
        let renamed = CONTEXT_WINDOW_STREAM.replace(
            r#""model":"claude-sonnet-4-6","content""#,
            r#""model":"claude-sonnet-4-6[1m]","content""#,
        );

        assert_eq!(
            occupancies(&interpret_all(&renamed)),
            [Occupancy::reported(1_000_000, None).expect("a window")]
        );
    }

    #[test]
    fn a_model_usage_of_another_shape_reports_no_window_and_the_turn_still_finishes() {
        let result = CONTEXT_WINDOW_STREAM
            .lines()
            .last()
            .expect("the result line");
        for usage in [
            json!([1, 2]),
            json!("wide"),
            json!({"claude-sonnet-4-6": 5}),
            json!({"claude-sonnet-4-6": {"contextWindow": "wide"}}),
            json!({"claude-sonnet-4-6": {"contextWindow": 0}}),
            json!({}),
        ] {
            let mut line: Value = serde_json::from_str(result).expect("a result");
            line["modelUsage"] = usage.clone();
            let mut events = Vec::new();
            let mut turn = Turn::default();
            for line in CONTEXT_WINDOW_STREAM
                .lines()
                .take_while(|line| !line.contains(r#""type":"result""#))
            {
                turn.interpret(line, &mut events);
            }
            turn.interpret(&line.to_string(), &mut events);

            assert!(occupancies(&events).is_empty(), "{usage}: {events:?}");
            assert!(
                matches!(events.last(), Some(AgentEvent::Finished { .. })),
                "{usage}: {events:?}"
            );
        }
    }

    /// A fill measured on one model says nothing of another's window, so a
    /// switch with no usage of its own leaves the new window unfilled.
    #[test]
    fn a_switch_of_model_without_usage_forgets_the_old_models_fill() {
        let result = CONTEXT_WINDOW_STREAM
            .lines()
            .last()
            .expect("the result line");
        let switched = json!({
            "type": "assistant",
            "message": {"model": "claude-fable-5-1", "role": "assistant", "content": [{"type": "text", "text": "Switched."}]},
            "parent_tool_use_id": null,
            "session_id": "6f1",
        });
        let mut events = Vec::new();
        let mut turn = Turn::default();
        for line in CONTEXT_WINDOW_STREAM
            .lines()
            .take_while(|line| !line.contains(r#""type":"result""#))
        {
            turn.interpret(line, &mut events);
        }
        turn.interpret(&switched.to_string(), &mut events);
        turn.interpret(result, &mut events);

        assert_eq!(
            occupancies(&events),
            [Occupancy::reported(1_000_000, None).expect("a window")]
        );
    }

    #[test]
    fn an_entry_of_another_shape_leaves_the_main_models_window() {
        let result = CONTEXT_WINDOW_STREAM
            .lines()
            .last()
            .expect("the result line");
        let mut line: Value = serde_json::from_str(result).expect("a result");
        line["modelUsage"]["claude-fable-5-1"] = json!(5);
        let mut events = Vec::new();
        let mut turn = Turn::default();
        for line in CONTEXT_WINDOW_STREAM
            .lines()
            .take_while(|line| !line.contains(r#""type":"result""#))
        {
            turn.interpret(line, &mut events);
        }
        turn.interpret(&line.to_string(), &mut events);

        assert_eq!(
            occupancies(&events),
            [Occupancy::reported(200_000, Some(16_632)).expect("a window")]
        );
    }

    #[test]
    fn a_result_without_model_usage_reports_no_window() {
        assert!(occupancies(&interpret_all(SESSION)).is_empty());
    }

    #[test]
    fn the_window_comes_before_the_event_that_ends_the_turn() {
        let events = interpret_all(CONTEXT_WINDOW_STREAM);
        let position = |wanted: fn(&AgentEvent) -> bool| {
            events
                .iter()
                .position(wanted)
                .expect("the event is reported")
        };

        let occupancy = position(|event| matches!(event, AgentEvent::Occupancy(_)));
        let finished = position(|event| matches!(event, AgentEvent::Finished { .. }));
        assert!(occupancy < finished, "{events:?}");
        assert_eq!(finished, events.len() - 1, "{events:?}");
    }

    #[test]
    fn a_failed_result_still_reports_its_window_before_the_failure() {
        let failed = CONTEXT_WINDOW_STREAM.replace(
            r#""subtype":"success","is_error":false"#,
            r#""subtype":"error_during_execution","is_error":true"#,
        );

        let events = interpret_all(&failed);

        assert_eq!(
            occupancies(&events),
            [Occupancy::reported(200_000, Some(16_632)).expect("a window")]
        );
        assert!(
            matches!(events.last(), Some(AgentEvent::Failed(_))),
            "{events:?}"
        );
    }
}
