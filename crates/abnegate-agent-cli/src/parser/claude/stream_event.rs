use std::time::Duration;

use abnegate_llm::Usage;
use serde::Deserialize;

use crate::event::AgentEvent;
use crate::parser::claude::cli_content_block::CliContentBlock;
use crate::parser::claude::cli_message::CliMessage;
use crate::parser::claude::cli_usage::CliUsage;
use crate::parser::claude::rate_limit_report::RateLimitReport;

/// The wording a throttled run is reported with.
///
/// A caller recognises a throttled run by the words in the failure, and finds
/// when to try again in the `"resetsAt"` of the report quoted after them, so
/// both are load-bearing and not decoration.
const THROTTLED: &str = "rate limit reached";

const FAILED: &str = "the agent reported a failed run";

const NO_ARGUMENTS: &str = "{}";

/// One line of `claude --output-format stream-json`.
///
/// A variant may gain a field in a minor release, so a pattern outside this
/// crate ends in `..`:
///
/// ```compile_fail,E0638
/// use abnegate_agent_cli::StreamEvent;
///
/// fn session(event: &StreamEvent) -> Option<&str> {
///     match event {
///         StreamEvent::System {
///             subtype: _,
///             session_id,
///         } => session_id.as_deref(),
///         _ => None,
///     }
/// }
/// # let _ = session;
/// ```
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum StreamEvent {
    #[serde(rename = "system")]
    #[non_exhaustive]
    System {
        #[serde(default)]
        subtype: Option<String>,
        #[serde(default)]
        session_id: Option<String>,
    },
    #[serde(rename = "assistant")]
    #[non_exhaustive]
    Assistant {
        #[serde(default)]
        message: Option<CliMessage>,
    },
    #[serde(rename = "user")]
    #[non_exhaustive]
    User {},
    #[serde(rename = "result")]
    #[non_exhaustive]
    Result {
        #[serde(default)]
        subtype: Option<String>,
        #[serde(default)]
        is_error: bool,
        /// The agent's final prose, or its own account of why it failed.
        #[serde(default)]
        result: Option<String>,
        /// The answer shaped to the schema passed with `--json-schema`.
        #[serde(default)]
        structured_output: Option<serde_json::Value>,
        #[serde(default)]
        total_cost_usd: Option<f64>,
        #[serde(default, rename = "num_turns")]
        turns: Option<i64>,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default, rename = "duration_api_ms")]
        api_milliseconds: Option<i64>,
        #[serde(default)]
        usage: Option<CliUsage>,
    },
    /// A throttling report. Anything but explicit headroom fails the run,
    /// a report with no status at all included: the event exists to
    /// announce a limit being hit, and waiting out a refused run costs far
    /// more than stopping one.
    #[serde(rename = "rate_limit_event")]
    #[non_exhaustive]
    RateLimit {
        /// The report itself, read from `rate_limit_info`.
        #[serde(default, rename = "rate_limit_info")]
        report: Option<RateLimitReport>,
        /// Where some releases put the reset time instead of inside `report`.
        #[serde(default, rename = "resetsAt")]
        resets_at: Option<serde_json::Value>,
    },
    #[serde(other)]
    Unknown,
}

impl StreamEvent {
    /// Append what this event means, whichever of it this crate understands.
    pub fn interpret(self, events: &mut Vec<AgentEvent>) {
        match self {
            Self::System { session_id, .. } => events.extend(session_id.map(AgentEvent::Session)),
            Self::Assistant {
                message: Some(message),
            } => assistant(message, events),
            Self::Result {
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
                events.push(conclusion(subtype, is_error, result));
            }
            Self::RateLimit { report, resets_at } => {
                let report = report.unwrap_or_default();
                if !report.allowed() {
                    events.push(AgentEvent::Failed(throttled(report, resets_at)));
                }
            }
            Self::Assistant { message: None } | Self::User {} | Self::Unknown => {}
        }
    }
}

fn assistant(message: CliMessage, events: &mut Vec<AgentEvent>) {
    for block in message.content {
        match block {
            CliContentBlock::Text { text } => events.push(AgentEvent::Text(text)),
            CliContentBlock::ToolUse { id, name, input } => {
                let arguments = if input.is_null() {
                    NO_ARGUMENTS.to_string()
                } else {
                    input.to_string()
                };
                events.push(AgentEvent::tool(id, name, arguments));
            }
            CliContentBlock::Other => {}
        }
    }
    if let Some(usage) = message.usage {
        events.push(AgentEvent::Usage(Usage::from(&usage)));
    }
}

fn conclusion(subtype: Option<String>, is_error: bool, result: Option<String>) -> AgentEvent {
    let failed = is_error
        || subtype
            .as_deref()
            .is_some_and(|subtype| subtype.starts_with("error"));
    if !failed {
        return AgentEvent::finished(subtype);
    }
    AgentEvent::Failed(
        result
            .filter(|result| !result.trim().is_empty())
            .or(subtype)
            .unwrap_or_else(|| FAILED.to_string()),
    )
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
    use serde_json::json;

    use super::StreamEvent;
    use crate::parser::claude::cli_content_block::CliContentBlock;

    fn parse(line: &str) -> StreamEvent {
        serde_json::from_str(line).expect("a stream event")
    }

    #[test]
    fn an_assistant_text_event_is_read() {
        let event = parse(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello world"}]}}"#,
        );
        let StreamEvent::Assistant {
            message: Some(message),
        } = event
        else {
            panic!("unexpected event: {event:?}");
        };
        assert_eq!(
            message.content,
            [CliContentBlock::Text {
                text: "hello world".to_string()
            }]
        );
    }

    #[test]
    fn an_assistant_tool_use_event_is_read() {
        let event = parse(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu_1","name":"Bash"}]}}"#,
        );
        let StreamEvent::Assistant {
            message: Some(message),
        } = event
        else {
            panic!("unexpected event: {event:?}");
        };
        assert!(matches!(
            &message.content[..],
            [CliContentBlock::ToolUse { id, name, .. }] if id == "tu_1" && name == "Bash"
        ));
    }

    #[test]
    fn several_blocks_in_one_assistant_event_keep_their_order() {
        let event = parse(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Running command..."},{"type":"tool_use","id":"tu_2","name":"Read"}]}}"#,
        );
        let StreamEvent::Assistant {
            message: Some(message),
        } = event
        else {
            panic!("unexpected event: {event:?}");
        };
        assert_eq!(message.content.len(), 2);
        assert!(
            matches!(&message.content[0], CliContentBlock::Text { text } if text == "Running command...")
        );
        assert!(
            matches!(&message.content[1], CliContentBlock::ToolUse { id, name, .. } if id == "tu_2" && name == "Read")
        );
    }

    #[test]
    fn an_unknown_block_is_other() {
        let event = parse(
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"}]}}"#,
        );
        let StreamEvent::Assistant {
            message: Some(message),
        } = event
        else {
            panic!("unexpected event: {event:?}");
        };
        assert_eq!(message.content, [CliContentBlock::Other]);
    }

    #[test]
    fn an_assistant_event_may_carry_no_message_or_no_content() {
        assert_eq!(
            parse(r#"{"type":"assistant"}"#),
            StreamEvent::Assistant { message: None }
        );

        let event = parse(r#"{"type":"assistant","message":{"content":[]}}"#);
        assert!(matches!(
            event,
            StreamEvent::Assistant { message: Some(ref message) } if message.content.is_empty()
        ));
    }

    #[test]
    fn system_and_user_events_ignore_their_extra_fields() {
        assert!(matches!(
            parse(r#"{"type":"system"}"#),
            StreamEvent::System { .. }
        ));
        assert_eq!(
            parse(
                r#"{"type":"system","subtype":"init","session_id":"xyz","extra_field":"ignored"}"#
            ),
            StreamEvent::System {
                subtype: Some("init".to_string()),
                session_id: Some("xyz".to_string()),
            }
        );
        assert_eq!(parse(r#"{"type":"user"}"#), StreamEvent::User {});
        assert_eq!(
            parse(r#"{"type":"user","message":{"role":"user","content":"hello"},"extra":true}"#),
            StreamEvent::User {}
        );
    }

    #[test]
    fn an_unknown_event_type_is_kept_for_forward_compatibility() {
        for line in [
            r#"{"type":"some_future_event","data":"anything"}"#,
            r#"{"type":"new_future_event"}"#,
        ] {
            assert_eq!(parse(line), StreamEvent::Unknown, "{line}");
        }
    }

    #[test]
    fn a_result_with_structured_output_is_read() {
        let event =
            parse(r#"{"type":"result","structured_output":{"summary":"done","success":true}}"#);
        let StreamEvent::Result {
            structured_output: Some(output),
            ..
        } = event
        else {
            panic!("unexpected event: {event:?}");
        };
        assert_eq!(output["summary"], "done");
        assert_eq!(output["success"], true);
    }

    #[test]
    fn a_bare_result_leaves_every_optional_field_absent() {
        let event = parse(r#"{"type":"result"}"#);
        assert_eq!(
            event,
            StreamEvent::Result {
                subtype: None,
                is_error: false,
                result: None,
                structured_output: None,
                total_cost_usd: None,
                turns: None,
                session_id: None,
                api_milliseconds: None,
                usage: None,
            }
        );
    }

    #[test]
    fn a_result_carries_its_cost_turns_session_timing_and_usage() {
        let event = parse(
            r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":2557,"duration_api_ms":2546,"num_turns":1,"result":"done","session_id":"sess-123","total_cost_usd":0.027,"usage":{"input_tokens":3,"cache_creation_input_tokens":2833,"cache_read_input_tokens":18758,"output_tokens":4}}"#,
        );
        let StreamEvent::Result {
            subtype,
            result,
            total_cost_usd: Some(cost),
            turns: Some(turns),
            session_id: Some(session),
            api_milliseconds: Some(milliseconds),
            usage: Some(usage),
            ..
        } = event
        else {
            panic!("unexpected event: {event:?}");
        };
        assert_eq!(subtype.as_deref(), Some("success"));
        assert_eq!(result.as_deref(), Some("done"));
        assert!((cost - 0.027).abs() < 1e-6);
        assert_eq!(turns, 1);
        assert_eq!(session, "sess-123");
        assert_eq!(milliseconds, 2546);
        assert_eq!(usage.input_tokens, Some(3));
        assert_eq!(usage.output_tokens, Some(4));
        assert_eq!(usage.cache_read_input_tokens, Some(18758));
        assert_eq!(usage.cache_creation_input_tokens, Some(2833));
    }

    #[test]
    fn every_result_field_is_read_from_a_pretty_printed_line() {
        let event = parse(
            r#"{
                "type": "result",
                "structured_output": {"summary": "all done", "success": true},
                "total_cost_usd": 0.123,
                "num_turns": 5,
                "session_id": "sess-abc",
                "duration_api_ms": 9876,
                "usage": {"input_tokens": 100, "output_tokens": 200, "cache_read_input_tokens": 300, "cache_creation_input_tokens": 400}
            }"#,
        );
        let StreamEvent::Result {
            structured_output: Some(output),
            total_cost_usd: Some(cost),
            turns: Some(5),
            session_id: Some(session),
            api_milliseconds: Some(9876),
            usage: Some(usage),
            ..
        } = event
        else {
            panic!("unexpected event: {event:?}");
        };
        assert_eq!(output["summary"], "all done");
        assert!((cost - 0.123).abs() < 1e-6);
        assert_eq!(session, "sess-abc");
        assert_eq!(usage.cache_creation_input_tokens, Some(400));
    }

    #[test]
    fn a_result_with_only_a_session_is_read() {
        let event = parse(r#"{"type":"result","session_id":"sess-xyz"}"#);
        assert!(matches!(
            event,
            StreamEvent::Result { session_id: Some(ref session), structured_output: None, total_cost_usd: None, turns: None, api_milliseconds: None, usage: None, .. } if session == "sess-xyz"
        ));
    }

    #[test]
    fn edge_values_in_a_result_still_parse() {
        let zero = parse(r#"{"type":"result","total_cost_usd":0.0,"num_turns":0}"#);
        assert!(matches!(
            zero,
            StreamEvent::Result { total_cost_usd: Some(cost), turns: Some(0), .. } if cost.abs() < 1e-10
        ));

        let large = parse(
            r#"{"type":"result","total_cost_usd":999.99,"num_turns":1000,"duration_api_ms":3600000,"usage":{"input_tokens":1000000,"output_tokens":500000}}"#,
        );
        let StreamEvent::Result {
            total_cost_usd: Some(cost),
            turns: Some(1000),
            api_milliseconds: Some(3_600_000),
            usage: Some(usage),
            ..
        } = large
        else {
            panic!("unexpected event: {large:?}");
        };
        assert!((cost - 999.99).abs() < 0.01);
        assert_eq!(usage.input_tokens, Some(1_000_000));
        assert_eq!(usage.output_tokens, Some(500_000));

        let negative = parse(r#"{"type":"result","duration_api_ms":-1,"total_cost_usd":-0.001}"#);
        assert!(matches!(
            negative,
            StreamEvent::Result { api_milliseconds: Some(-1), total_cost_usd: Some(cost), .. } if cost < 0.0
        ));

        let tokens = parse(
            r#"{"type":"result","usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}"#,
        );
        assert!(matches!(
            tokens,
            StreamEvent::Result { usage: Some(ref usage), .. } if usage.input_tokens == Some(0) && usage.cache_creation_input_tokens == Some(0)
        ));
    }

    #[test]
    fn extra_result_fields_are_ignored() {
        let event = parse(
            r#"{"type":"result","subtype":"success","is_error":false,"result":"text","extra":"ignored"}"#,
        );
        assert!(matches!(
            event,
            StreamEvent::Result { structured_output: None, ref result, .. } if result.as_deref() == Some("text")
        ));
    }

    #[test]
    fn a_rate_limit_event_is_read_with_its_report() {
        let event = parse(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","utilization":0.5}}"#,
        );
        let StreamEvent::RateLimit {
            report: Some(report),
            ..
        } = event
        else {
            panic!("unexpected event: {event:?}");
        };
        assert_eq!(report.status.as_deref(), Some("allowed"));
        assert!((report.utilization.expect("a utilisation") - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn a_rate_limit_event_may_carry_its_reset_at_the_top_level() {
        let event = parse(r#"{"type":"rate_limit_event","resetsAt":"2026-02-23T06:00:00Z"}"#);
        assert_eq!(
            event,
            StreamEvent::RateLimit {
                report: None,
                resets_at: Some(json!("2026-02-23T06:00:00Z")),
            }
        );
    }
}
