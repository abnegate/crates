//! Reading `claude --output-format stream-json`.

mod cli_content_block;
mod cli_message;
mod cli_usage;
mod rate_limit_info;
mod stream_event;

pub use crate::parser::claude::cli_content_block::CliContentBlock;
pub use crate::parser::claude::cli_message::CliMessage;
pub use crate::parser::claude::cli_usage::CliUsage;
pub use crate::parser::claude::rate_limit_info::RateLimitInfo;
pub use crate::parser::claude::stream_event::StreamEvent;

use serde_json::Value;

use crate::event::AgentEvent;
use crate::parser;

const RESULT: &str = "result";
const STRUCTURED_OUTPUT: &str = "structured_output";
const ENDS_TURN: &str = "result";
const ASSISTANT: &str = "assistant";
const TEXT: &str = "text";
const PARTIAL: &str = "stream_event";

/// How deep a content block's own `"type"` sits: in the block, in the
/// message's `content` array, in the message, in the event.
const BLOCK_DEPTH: usize = 4;

/// Translate one line of the stream, appending whatever it means.
pub fn interpret(line: &str, events: &mut Vec<AgentEvent>) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    match serde_json::from_str::<StreamEvent>(line) {
        Ok(event) => event.interpret(events),
        Err(_) => salvage(line, events),
    }
}

/// Whether `line` is a partial message, which `--include-partial-messages`
/// adds: a piece of an event the stream repeats whole once it is complete.
pub fn partial(line: &str) -> bool {
    parser::kind(line) == Some(PARTIAL)
}

/// Whether an event too long to read, of which only `prefix` is known, is
/// one the run cannot do without: the result that ends the turn, or a reply
/// that holds prose or whose first block is not known yet. A reply known to
/// hold only a tool call or thinking is dropped like a tool result, since the
/// agent makes the call whether or not it is read here.
pub fn essential(prefix: &str) -> bool {
    let types = parser::types(prefix);
    let top = |kind: &str| {
        types
            .iter()
            .any(|&(depth, found)| depth == 1 && found == kind)
    };
    if top(ENDS_TURN) {
        return true;
    }
    let mut blocks = types
        .iter()
        .filter(|(depth, _)| *depth == BLOCK_DEPTH)
        .map(|(_, kind)| *kind)
        .peekable();
    top(ASSISTANT) && (blocks.peek().is_none() || blocks.any(|kind| kind == TEXT))
}

/// A line that is not a stream event can still be the CLI's final JSON
/// wrapper, carrying the schema-shaped answer either as a JSON document inside
/// its `result` string or directly as `structured_output`.
fn salvage(line: &str, events: &mut Vec<AgentEvent>) {
    let Ok(Value::Object(wrapper)) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let embedded = wrapper
        .get(RESULT)
        .and_then(Value::as_str)
        .and_then(|result| serde_json::from_str::<Value>(result).ok());
    let structured = embedded
        .into_iter()
        .chain(wrapper.get(STRUCTURED_OUTPUT).cloned())
        .find(Value::is_object);
    events.extend(structured.map(AgentEvent::Structured));
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::CliUsage;
    use super::essential;
    use super::interpret;
    use super::partial;
    use crate::event::AgentEvent;

    /// Recorded from `claude --verbose --output-format stream-json --print`.
    const SESSION: &str = r#"{"type":"system","subtype":"init","cwd":"/w","session_id":"6f1","tools":["Read","Edit"],"model":"claude-opus-4","permissionMode":"default","apiKeySource":"none"}
{"type":"assistant","message":{"id":"msg_014","type":"message","role":"assistant","model":"claude-opus-4","content":[{"type":"text","text":"Reading the file first."}],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":4,"cache_creation_input_tokens":1200,"cache_read_input_tokens":8400,"output_tokens":7}},"session_id":"6f1"}
{"type":"assistant","message":{"id":"msg_015","type":"message","role":"assistant","model":"claude-opus-4","content":[{"type":"tool_use","id":"toolu_01A","name":"Read","input":{"file_path":"/w/main.rs"}}],"stop_reason":"tool_use","usage":{"input_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":9600,"output_tokens":58}},"session_id":"6f1"}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_01A","content":"fn main() {}"}]},"session_id":"6f1"}
{"type":"assistant","message":{"id":"msg_016","type":"message","role":"assistant","model":"claude-opus-4","content":[{"type":"text","text":"The entry point is empty."}],"stop_reason":"end_turn","usage":{"input_tokens":3,"cache_read_input_tokens":9700,"output_tokens":12}},"session_id":"6f1"}
{"type":"result","subtype":"success","is_error":false,"duration_ms":8421,"duration_api_ms":7980,"num_turns":3,"result":"The entry point is empty.","session_id":"6f1","total_cost_usd":0.0412,"usage":{"input_tokens":9,"cache_creation_input_tokens":1200,"cache_read_input_tokens":27700,"output_tokens":77}}"#;

    fn interpret_all(sample: &str) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        for line in sample.lines() {
            interpret(line, &mut events);
        }
        events
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

    #[test]
    fn a_recorded_session_yields_its_text_tools_and_result() {
        let events = interpret_all(SESSION);

        assert_eq!(
            text(&events),
            "Reading the file first.The entry point is empty."
        );

        let tools: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Tool(call) => Some(call),
                _ => None,
            })
            .collect();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].id, "toolu_01A");
        assert_eq!(tools[0].call_type, "function");
        assert_eq!(tools[0].function.name, "Read");
        assert_eq!(tools[0].function.arguments, r#"{"file_path":"/w/main.rs"}"#);

        let last = events.last().expect("a terminal event");
        assert!(
            matches!(last, AgentEvent::Finished { finish_reason } if finish_reason.as_deref() == Some("success")),
            "expected a successful finish, got {last:?}"
        );
    }

    #[test]
    fn a_recorded_session_reports_its_session_cost_turns_and_timing() {
        let events = interpret_all(SESSION);

        let sessions: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Session(session) => Some(session.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(sessions, ["6f1", "6f1"], "from the init and the result");

        assert!(
            events.iter().any(
                |event| matches!(event, AgentEvent::Cost(cost) if (cost - 0.0412).abs() < 1e-9)
            )
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::Turns(3)))
        );
        assert!(events.iter().any(
            |event| matches!(event, AgentEvent::Latency(latency) if *latency == Duration::from_millis(7980))
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::Tokens(CliUsage {
                input_tokens: Some(9),
                output_tokens: Some(77),
                cache_read_input_tokens: Some(27_700),
                cache_creation_input_tokens: Some(1_200),
            })
        )));
    }

    #[test]
    fn cached_prompt_tokens_are_counted_as_prompt_tokens() {
        let events = interpret_all(SESSION);

        let usage = events
            .iter()
            .rev()
            .find_map(|event| match event {
                AgentEvent::Usage(usage) => Some(usage),
                _ => None,
            })
            .expect("a usage event");

        assert_eq!(usage.prompt_tokens, 9 + 1200 + 27700);
        assert_eq!(usage.completion_tokens, 77);
        assert_eq!(usage.total_tokens, 9 + 1200 + 27700 + 77);
    }

    #[test]
    fn a_tool_call_without_input_carries_an_empty_object() {
        let mut events = Vec::new();
        interpret(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu_1","name":"Bash"}]}}"#,
            &mut events,
        );

        let [AgentEvent::Tool(call)] = events.as_slice() else {
            panic!("expected one tool call, got {events:?}");
        };
        assert_eq!(call.function.arguments, "{}");
    }

    #[test]
    fn a_headroom_report_is_not_a_failure() {
        for status in ["allowed", "allowed_warning"] {
            let line = format!(
                r#"{{"type":"rate_limit_event","rate_limit_info":{{"status":"{status}","rateLimitType":"five_hour","utilization":72.5}}}}"#
            );
            let mut events = Vec::new();
            interpret(&line, &mut events);
            assert!(events.is_empty(), "{status} was treated as a failure");
        }
    }

    #[test]
    fn a_rate_limit_event_with_no_report_at_all_is_a_refusal() {
        for line in [
            r#"{"type":"rate_limit_event"}"#,
            r#"{"type":"rate_limit_event","rate_limit_info":{}}"#,
            r#"{"type":"rate_limit_event","rate_limit_info":{"rateLimitType":"five_hour"}}"#,
        ] {
            let mut events = Vec::new();
            interpret(line, &mut events);

            let [AgentEvent::Failed(message)] = events.as_slice() else {
                panic!("expected a refusal for {line}, got {events:?}");
            };
            assert!(message.starts_with("rate limit reached: {"), "{message}");
        }
    }

    #[test]
    fn a_refused_request_reads_as_a_rate_limit_to_the_caller() {
        let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"seven_day"}}"#;
        let mut events = Vec::new();
        interpret(line, &mut events);

        let [AgentEvent::Failed(message)] = events.as_slice() else {
            panic!("expected one failure, got {events:?}");
        };
        assert!(
            message.to_ascii_lowercase().contains("rate limit"),
            "the caller cannot classify {message:?}"
        );
        assert!(message.contains("seven_day"));
        assert!(message.contains("rejected"));
    }

    #[test]
    fn a_refusal_quotes_its_report_with_the_reset_wherever_the_cli_put_it() {
        for line in [
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"exceeded","resetsAt":"2026-02-23T06:00:00Z","rateLimitType":"seven_day","utilization":1.0}}"#,
            r#"{"type":"rate_limit_event","resetsAt":"2026-02-23T06:00:00Z"}"#,
        ] {
            let mut events = Vec::new();
            interpret(line, &mut events);

            let [AgentEvent::Failed(message)] = events.as_slice() else {
                panic!("expected one failure for {line}, got {events:?}");
            };
            assert!(message.starts_with("rate limit reached: {"), "{message}");
            assert!(
                message.contains(r#""resetsAt":"2026-02-23T06:00:00Z""#),
                "{message}"
            );
        }

        let mut events = Vec::new();
        interpret(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1772096400}}"#,
            &mut events,
        );
        assert!(matches!(
            events.as_slice(),
            [AgentEvent::Failed(message)] if message.contains(r#""resetsAt":1772096400"#)
        ));
    }

    #[test]
    fn a_failed_result_carries_the_agents_own_wording() {
        let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Invalid API key provided","session_id":"6f1"}"#;
        let mut events = Vec::new();
        interpret(line, &mut events);

        let [AgentEvent::Session(_), AgentEvent::Failed(message)] = events.as_slice() else {
            panic!("expected a session and a failure, got {events:?}");
        };
        assert_eq!(message, "Invalid API key provided");
    }

    #[test]
    fn a_failed_result_without_wording_falls_back_to_its_subtype() {
        let line =
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":"   "}"#;
        let mut events = Vec::new();
        interpret(line, &mut events);

        let [AgentEvent::Failed(message)] = events.as_slice() else {
            panic!("expected one failure, got {events:?}");
        };
        assert_eq!(message, "error_max_turns");
    }

    #[test]
    fn a_failed_result_with_no_wording_at_all_still_reports_a_failure() {
        let mut events = Vec::new();
        interpret(r#"{"type":"result","is_error":true}"#, &mut events);

        assert!(matches!(
            events.as_slice(),
            [AgentEvent::Failed(message)] if message == "the agent reported a failed run"
        ));
    }

    #[test]
    fn a_success_subtype_carrying_an_error_flag_is_still_a_failure() {
        let line = r#"{"type":"result","subtype":"success","is_error":true,"result":"You have hit your weekly limit"}"#;
        let mut events = Vec::new();
        interpret(line, &mut events);

        assert!(matches!(events.as_slice(), [AgentEvent::Failed(_)]));
    }

    #[test]
    fn a_structured_answer_is_reported_before_the_finish() {
        let mut events = Vec::new();
        interpret(
            r#"{"type":"result","subtype":"success","structured_output":{"summary":"Fixed the bug","success":true}}"#,
            &mut events,
        );

        let [AgentEvent::Structured(output), AgentEvent::Finished { .. }] = events.as_slice()
        else {
            panic!("expected a structured answer then a finish, got {events:?}");
        };
        assert_eq!(output["summary"], "Fixed the bug");
    }

    #[test]
    fn a_negative_count_or_duration_is_dropped_rather_than_failing_the_result() {
        let mut events = Vec::new();
        interpret(
            r#"{"type":"result","subtype":"success","num_turns":-1,"duration_api_ms":-1}"#,
            &mut events,
        );

        assert!(matches!(events.as_slice(), [AgentEvent::Finished { .. }]));
    }

    #[test]
    fn a_final_wrapper_with_a_json_result_is_salvaged_as_the_structured_answer() {
        let wrapper = serde_json::json!({ "result": r#"{"summary":"done","success":true}"# });
        let mut events = Vec::new();
        interpret(&wrapper.to_string(), &mut events);

        let [AgentEvent::Structured(output)] = events.as_slice() else {
            panic!("expected a structured answer, got {events:?}");
        };
        assert_eq!(output["summary"], "done");
        assert_eq!(output["success"], true);
    }

    #[test]
    fn a_final_wrapper_with_structured_output_is_salvaged() {
        let wrapper =
            serde_json::json!({ "structured_output": {"summary": "fixed", "success": true} });
        let mut events = Vec::new();
        interpret(&wrapper.to_string(), &mut events);

        let [AgentEvent::Structured(output)] = events.as_slice() else {
            panic!("expected a structured answer, got {events:?}");
        };
        assert_eq!(output["summary"], "fixed");
    }

    #[test]
    fn a_wrapper_whose_result_is_prose_falls_back_to_its_structured_output() {
        let wrapper = serde_json::json!({
            "result": "All done.",
            "structured_output": {"summary": "fixed", "success": true}
        });
        let mut events = Vec::new();
        interpret(&wrapper.to_string(), &mut events);

        assert!(matches!(
            events.as_slice(),
            [AgentEvent::Structured(output)] if output["summary"] == "fixed"
        ));
    }

    #[test]
    fn a_realistic_stream_yields_its_text_structured_answer_cost_and_turns() {
        let events = interpret_all(
            r#"{"type":"system","subtype":"init","session_id":"sess-001"}
{"type":"assistant","message":{"content":[{"type":"text","text":"I'll analyze the issue."}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu_1","name":"Bash"}]}}
{"type":"user","message":{"role":"user"}}
{"type":"assistant","message":{"content":[{"type":"text","text":"The fix is applied."}]}}
{"type":"result","structured_output":{"summary":"Fixed the bug","success":true,"pr_url":"https://github.com/org/repo/pull/42"},"total_cost_usd":0.05,"num_turns":3,"session_id":"sess-001","duration_api_ms":15000,"usage":{"input_tokens":1000,"output_tokens":500,"cache_read_input_tokens":2000,"cache_creation_input_tokens":100}}"#,
        );

        assert_eq!(text(&events), "I'll analyze the issue.The fix is applied.");
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::Structured(output) if output["pr_url"] == "https://github.com/org/repo/pull/42"
        )));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::Cost(cost) if (cost - 0.05).abs() < 1e-6))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::Turns(3)))
        );
        assert!(matches!(events.last(), Some(AgentEvent::Finished { .. })));
    }

    #[test]
    fn only_prose_or_a_result_is_essential_when_too_long_to_read() {
        for prefix in [
            r#"{"type":"assistant","message":{"id":"msg_1","type":"message","content":[{"type":"text","text":"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"#,
        ] {
            assert!(essential(prefix), "{prefix}");
        }
        for prefix in [
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":[{"type":"image","source":{"type":"base64","data":"iVBOR"#,
            r#"{"type":"system","subtype":"init","tools":["#,
            r#"{"type":"user","message":{"content":"{\"type\":\"result\"}"#,
            "xxxxxxxxxxxxxxxx",
        ] {
            assert!(!essential(prefix), "{prefix}");
        }
    }

    #[test]
    fn a_reply_too_long_to_read_is_essential_only_when_it_may_hold_prose() {
        for prefix in [
            r#"{"type":"assistant","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-opus-4","content":["#,
            r#"{"type":"assistant","message":{"id":"msg_1","model":"claude-opus-"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Read","input":{}},{"type":"text","text":"#,
        ] {
            assert!(essential(prefix), "{prefix}");
        }
        for prefix in [
            r#"{"type":"assistant","message":{"id":"msg_1","type":"message","content":[{"type":"tool_use","id":"toolu_1","name":"Write","input":{"file_path":"/w/a.rs","content":"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Write","input":{"blocks":[{"type":"text","text":"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"#,
        ] {
            assert!(!essential(prefix), "{prefix}");
        }
    }

    #[test]
    fn only_a_stream_event_is_a_partial_message() {
        assert!(partial(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Xk9"}},"session_id":"6f1"}"#
        ));
        for line in SESSION.lines().chain([
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"{\"type\":\"stream_event\"}"}]}}"#,
            "not json",
        ]) {
            assert!(!partial(line), "{line}");
        }
    }

    #[test]
    fn unknown_events_and_noise_are_skipped() {
        let mut events = Vec::new();
        for line in [
            r#"{"type":"system","subtype":"init"}"#,
            r#"{"type":"user","message":{"role":"user","content":[]}}"#,
            r#"{"type":"invented_next_release","payload":{}}"#,
            r#"{"result":"not json either"}"#,
            r#"{"result":"42"}"#,
            "[not json",
            "",
            "   ",
        ] {
            interpret(line, &mut events);
        }
        assert!(events.is_empty(), "noise produced {events:?}");
    }
}
