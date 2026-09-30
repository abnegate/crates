use std::time::Duration;

use abnegate_llm::Usage;

use crate::event::AgentEvent;
use crate::parser::claude::CREDITS_UNCONFIRMED;
use crate::parser::claude::cli_content_block::CliContentBlock;
use crate::parser::claude::cli_message::CliMessage;
use crate::parser::claude::credits_refusal::CreditsRefusal;
use crate::parser::claude::rate_limit_report::RateLimitReport;
use crate::parser::claude::salvage;
use crate::parser::claude::stream_event::StreamEvent;

/// The wording a throttled run is reported with.
///
/// A caller recognises a throttled run by the words in the failure, and finds
/// when to try again in the `"resetsAt"` of the report quoted after them, so
/// both are load-bearing and not decoration.
const THROTTLED: &str = "rate limit reached";

const FAILED: &str = "the agent reported a failed run";

const FAILED_SUBTYPE_PREFIX: &str = "error";

const NO_ARGUMENTS: &str = "{}";

const UNNAMED_WINDOW: &str = "request";

/// The window claude reports refused when an allowance that only usage
/// credits extend is spent. The line after it types the refusal.
const OVERAGE_INCLUDED_WINDOW: &str = "seven_day_overage_included";

/// Why claude could not look the account's usage credits up, which a later
/// attempt may get past.
const INDETERMINATE_REASONS: [&str; 2] = ["fetch_error", "unknown"];

/// One turn of `claude --output-format stream-json`, read a line at a time.
///
/// A line's meaning can hang on the lines before it. claude reports a
/// refused request on a `rate_limit_event` that names no agent, then writes
/// the refusal as the line of the agent whose request it was, so the window
/// is remembered and fails the turn only on the main agent's API error or a
/// failed result. A line the main agent writes past it shows its own request
/// went through, and the window was a subagent's.
///
/// A subagent's line contributes only the tools it calls: its words, its
/// token counts and its API errors are its own conversation's, and claude
/// hands its failure to the main agent as the result of the call that
/// started it.
#[derive(Debug, Clone, Default)]
pub struct Turn {
    /// The latest reason claude gave for the account not spending usage
    /// credits.
    reason: Option<String>,
    /// The latest refused window, as the failure it would be.
    refusal: Option<String>,
    /// The main agent's failed request, which the result after it repeats.
    failure: Option<String>,
    /// Whether the turn has said it runs on usage credits.
    credited: bool,
}

impl Turn {
    /// Translate one line of the stream, appending whatever it means beside
    /// the lines before it.
    pub fn interpret(&mut self, line: &str, events: &mut Vec<AgentEvent>) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        match serde_json::from_str::<StreamEvent>(line) {
            Ok(event) => self.read(event, events),
            Err(_) => salvage(line, events),
        }
    }

    pub(crate) fn read(&mut self, event: StreamEvent, events: &mut Vec<AgentEvent>) {
        match event {
            StreamEvent::System { session_id, .. } => {
                events.extend(session_id.map(AgentEvent::Session));
            }
            StreamEvent::Assistant {
                message,
                parent_tool_use_id: Some(_),
                ..
            } => subagent(message, events),
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
                structured_output,
                total_cost_usd,
                turns,
                session_id,
                api_milliseconds,
                usage,
            } => {
                events.extend(session_id.map(AgentEvent::Session));
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
                events.push(self.conclusion(subtype, is_error, result));
            }
            StreamEvent::RateLimit { report, resets_at } => {
                self.limit(report.unwrap_or_default(), resets_at, events);
            }
            StreamEvent::User {} | StreamEvent::Unknown => {}
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
            let failure = match kind {
                Some(kind) => Some(self.refused(kind.marker(), &words)),
                None => self.refusal.as_deref().map(|window| worded(window, &words)),
            };
            if let Some(failure) = failure {
                self.failure = Some(failure.clone());
                events.push(AgentEvent::Failed(failure));
            }
            return;
        }
        self.refusal = None;
        let Some(message) = message else {
            return;
        };
        for block in message.content {
            match block {
                CliContentBlock::Text { text } => events.push(AgentEvent::Text(text)),
                CliContentBlock::ToolUse { id, name, input } => {
                    events.push(tool(id, name, &input));
                }
                CliContentBlock::Other => {}
            }
        }
        if let Some(usage) = message.usage {
            events.push(AgentEvent::Usage(Usage::from(&usage)));
        }
    }

    fn conclusion(
        &self,
        subtype: Option<String>,
        is_error: bool,
        result: Option<String>,
    ) -> AgentEvent {
        let failed = is_error
            || subtype
                .as_deref()
                .is_some_and(|subtype| subtype.starts_with(FAILED_SUBTYPE_PREFIX));
        if !failed {
            return AgentEvent::finished(subtype);
        }
        if let Some(failure) = &self.failure {
            return AgentEvent::Failed(failure.clone());
        }
        let words = result.filter(|result| !result.trim().is_empty());
        AgentEvent::Failed(match (words, &self.refusal) {
            (Some(words), Some(window)) => worded(window, &words),
            (Some(words), None) => words,
            (None, Some(window)) => window.clone(),
            (None, None) => subtype.unwrap_or_else(|| FAILED.to_string()),
        })
    }

    fn limit(
        &mut self,
        report: RateLimitReport,
        resets_at: Option<serde_json::Value>,
        events: &mut Vec<AgentEvent>,
    ) {
        self.reason.clone_from(&report.overage_disabled_reason);
        if report.is_using_overage == Some(true) && !self.credited {
            self.credited = true;
            let window = report.kind.as_deref().unwrap_or(UNNAMED_WINDOW);
            events.push(AgentEvent::Credits(window.to_string()));
        }
        if report.allowed()
            || report.error_code.is_some()
            || report.kind.as_deref() == Some(OVERAGE_INCLUDED_WINDOW)
        {
            return;
        }
        let refusal = throttled(report, resets_at);
        events.push(AgentEvent::Diagnostic(refusal.clone()));
        self.refusal = Some(refusal);
    }

    /// claude's `words` for a turn usage credits could not fund, begun with
    /// `marker`, or with [`CREDITS_UNCONFIRMED`] when claude could not look
    /// the account's usage credits up for the request it refused.
    fn refused(&self, marker: &str, words: &str) -> String {
        let marker = match self.reason.as_deref() {
            Some(reason) if INDETERMINATE_REASONS.contains(&reason) => {
                format!("{CREDITS_UNCONFIRMED} ({reason})")
            }
            _ => marker.to_string(),
        };
        worded(&marker, words)
    }
}

/// A line of a subagent's own conversation, of which only the tools it
/// calls are the turn's.
fn subagent(message: Option<CliMessage>, events: &mut Vec<AgentEvent>) {
    for block in message.into_iter().flat_map(|message| message.content) {
        if let CliContentBlock::ToolUse { id, name, input } = block {
            events.push(tool(id, name, &input));
        }
    }
}

fn tool(id: String, name: String, input: &serde_json::Value) -> AgentEvent {
    let arguments = if input.is_null() {
        NO_ARGUMENTS.to_string()
    } else {
        input.to_string()
    };
    AgentEvent::tool(id, name, arguments)
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

fn throttled(mut report: RateLimitReport, resets_at: Option<serde_json::Value>) -> String {
    if report.resets_at.is_none() {
        report.resets_at = resets_at;
    }
    let quoted = serde_json::to_string(&report).unwrap_or_default();
    format!("{THROTTLED}: {quoted}")
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use serde_json::json;

    use super::THROTTLED;
    use super::Turn;
    use crate::event::AgentEvent;
    use crate::parser::claude::CREDITS_REQUIRED;
    use crate::parser::claude::CREDITS_UNCONFIRMED;
    use crate::parser::claude::LONG_CONTEXT_CREDITS_REQUIRED;
    use crate::parser::claude::rate_limit_report::RateLimitReport;
    use crate::stdout_parse_result::StdoutParseResult;

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

    fn failures(events: &[AgentEvent]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Failed(message) => Some(message.as_str()),
                _ => None,
            })
            .collect()
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
        json!({
            "type": "result",
            "subtype": "success",
            "is_error": true,
            "api_error_status": 429,
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

    /// The failure a refused window `info` reads as, in claude's `words`.
    fn throttled(info: &Value, words: &str) -> String {
        let report: RateLimitReport =
            serde_json::from_value(info.clone()).expect("a rate limit report");
        let quoted = serde_json::to_string(&report).expect("serialisable");
        format!("{THROTTLED}: {quoted}: {words}")
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
            }
        }
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
            failed_result(OVERLOADED).to_string(),
        ]));

        assert_eq!(failures(&events), [OVERLOADED]);
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

    /// A subagent's words are its own conversation's. What it reaches for is
    /// still the turn's work, shown as the main agent's calls are.
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
        assert_eq!(tools(&events), ["Agent", "Read"]);
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

    /// Nothing here names a refusal by the words it is written in: a failed
    /// result keeps them, whatever they resemble.
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
}
