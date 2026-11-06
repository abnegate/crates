use std::collections::BTreeMap;

use serde::Deserialize;

use crate::event::AgentEvent;
use crate::parser::claude::cli_message::CliMessage;
use crate::parser::claude::cli_usage::CliUsage;
use crate::parser::claude::model_usage::ModelUsage;
use crate::parser::claude::rate_limit_report::RateLimitReport;
use crate::parser::claude::turn::Turn;
use crate::parser::lenient;

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
    /// A notice from the CLI itself, such as `init` when a session starts.
    #[serde(rename = "system")]
    #[non_exhaustive]
    System {
        /// What kind of notice it is.
        #[serde(default)]
        subtype: Option<String>,
        /// The session the run belongs to, which resumes it.
        #[serde(default)]
        session_id: Option<String>,
    },
    /// A message the model wrote: prose, tool calls, or both.
    #[serde(rename = "assistant")]
    #[non_exhaustive]
    Assistant {
        /// The message, when the event carries one.
        #[serde(default)]
        message: Option<CliMessage>,
        /// The call that started the subagent whose line this is, or `None`
        /// on the main agent's own line.
        #[serde(default)]
        parent_tool_use_id: Option<String>,
        /// Whether the message is the CLI's account of a failed request
        /// rather than anything the model said.
        #[serde(default)]
        is_api_error_message: bool,
        /// The CLI's own kind for that failure, such as
        /// `model_requires_usage_credits`.
        #[serde(default)]
        api_error: Option<String>,
    },
    /// What the agent's tools printed, fed back to the model.
    #[serde(rename = "user")]
    #[non_exhaustive]
    User {
        /// The message, whose [`ToolResult`](crate::CliContentBlock::ToolResult)
        /// blocks each answer one of the agent's calls. A message of another
        /// shape, such as a prompt's plain text, reads as absent rather than
        /// losing the whole event.
        #[serde(default, deserialize_with = "lenient")]
        message: Option<CliMessage>,
        /// The tool's own account of the call the event answers, read from
        /// `tool_use_result` or `toolUseResult`, in a shape of the tool's
        /// own: the report of the subagent an Agent call started, say.
        #[serde(default, alias = "toolUseResult", deserialize_with = "lenient")]
        tool_use_result: Option<serde_json::Value>,
    },
    /// The run's final event: its answer or failure, and what it cost.
    #[serde(rename = "result")]
    #[non_exhaustive]
    Result {
        /// How the run ended, such as `success` or `error_max_turns`.
        #[serde(default)]
        subtype: Option<String>,
        /// Whether the run ended in failure.
        #[serde(default)]
        is_error: bool,
        /// The agent's final prose, or its own account of why it failed.
        #[serde(default)]
        result: Option<String>,
        /// What went wrong, for a failure such as `error_max_turns` that
        /// carries no `result` text: each a sentence, or a JSON value of
        /// another shape. A list of any other shape reads as absent rather
        /// than losing the whole result.
        #[serde(default, deserialize_with = "lenient")]
        errors: Option<Vec<serde_json::Value>>,
        /// The answer shaped to the schema passed with `--json-schema`.
        #[serde(default)]
        structured_output: Option<serde_json::Value>,
        /// What the run cost, in US dollars.
        #[serde(default)]
        total_cost_usd: Option<f64>,
        /// How many turns the run took, read from `num_turns`.
        #[serde(default, rename = "num_turns")]
        turns: Option<i64>,
        /// The session the run belongs to, which resumes it.
        #[serde(default)]
        session_id: Option<String>,
        /// Milliseconds spent waiting on the model's API, read from
        /// `duration_api_ms`.
        #[serde(default, rename = "duration_api_ms")]
        api_milliseconds: Option<i64>,
        /// The tokens the whole run cost.
        #[serde(default)]
        usage: Option<CliUsage>,
        /// The HTTP status of the request that failed the run, such as 429
        /// for one a usage limit refused. A value of any other shape reads as
        /// absent rather than losing the whole result.
        #[serde(default, deserialize_with = "lenient")]
        api_error_status: Option<u16>,
        /// What the run used of each model it ran on, by the model's name,
        /// read from `modelUsage`. A value of any other shape reads as
        /// absent rather than losing the whole result.
        #[serde(default, rename = "modelUsage", deserialize_with = "lenient")]
        model_usage: Option<BTreeMap<String, ModelUsage>>,
    },
    /// A throttling report. Headroom on a named window of the plan, with how
    /// much of it is spent, is a [`AgentEvent::Window`]. Anything but
    /// [headroom](RateLimitReport::allowed) is a refused window, a report
    /// with no status at all included, since the event exists to announce a
    /// limit being hit. The event names no agent, so the window ends the turn
    /// in a [`AgentEvent::Limited`] only when the main agent's next line is a
    /// failed request, or the turn's result a failure.
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
    /// Any event this crate does not read.
    #[serde(other)]
    Unknown,
}

impl StreamEvent {
    /// Append what this event means on its own, as the first line of a
    /// [`Turn`](crate::parser::claude::Turn). A refused window waits for the
    /// line after it, so here it is only a
    /// [diagnostic](AgentEvent::Diagnostic).
    pub fn interpret(self, events: &mut Vec<AgentEvent>) {
        Turn::default().read(self, events);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::StreamEvent;
    use crate::event::AgentEvent;
    use crate::parser::claude::cli_content_block::CliContentBlock;
    use crate::parser::claude::model_usage::ModelUsage;

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
            ..
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
            ..
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
            ..
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
            ..
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
            StreamEvent::Assistant {
                message: None,
                parent_tool_use_id: None,
                is_api_error_message: false,
                api_error: None,
            }
        );

        let event = parse(r#"{"type":"assistant","message":{"content":[]}}"#);
        assert!(matches!(
            event,
            StreamEvent::Assistant { message: Some(ref message), .. } if message.content.is_empty()
        ));
    }

    #[test]
    fn a_subagents_api_error_line_names_its_parent_and_its_kind() {
        let event = parse(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Usage credits required."}]},"parent_tool_use_id":"toolu_01Agent","is_api_error_message":true,"api_error":"model_requires_usage_credits"}"#,
        );
        let StreamEvent::Assistant {
            parent_tool_use_id,
            is_api_error_message,
            api_error,
            ..
        } = event
        else {
            panic!("unexpected event: {event:?}");
        };
        assert_eq!(parent_tool_use_id.as_deref(), Some("toolu_01Agent"));
        assert!(is_api_error_message);
        assert_eq!(api_error.as_deref(), Some("model_requires_usage_credits"));
    }

    #[test]
    fn a_lone_refused_window_is_remembered_rather_than_failed() {
        let mut events = Vec::new();
        parse(r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour"}}"#)
            .interpret(&mut events);

        assert!(
            matches!(events.as_slice(), [AgentEvent::Diagnostic(message)] if message == "rate limit reached (five_hour, rejected)"),
            "{events:?}"
        );
    }

    #[test]
    fn a_failed_results_status_is_read_and_a_status_of_another_shape_is_absent() {
        for (line, expected) in [
            (
                r#"{"type":"result","is_error":true,"api_error_status":429}"#,
                Some(429),
            ),
            (
                r#"{"type":"result","is_error":true,"api_error_status":"429"}"#,
                None,
            ),
            (
                r#"{"type":"result","is_error":true,"api_error_status":null}"#,
                None,
            ),
            (
                r#"{"type":"result","is_error":true,"api_error_status":70000}"#,
                None,
            ),
        ] {
            let StreamEvent::Result {
                api_error_status, ..
            } = parse(line)
            else {
                panic!("{line} was not read as a result");
            };
            assert_eq!(api_error_status, expected, "{line}");
        }
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
        assert_eq!(
            parse(r#"{"type":"user"}"#),
            StreamEvent::User {
                message: None,
                tool_use_result: None,
            }
        );
        assert_eq!(
            parse(r#"{"type":"user","message":{"role":"user","content":"hello"},"extra":true}"#),
            StreamEvent::User {
                message: None,
                tool_use_result: None,
            }
        );
    }

    #[test]
    fn a_user_event_carries_its_tool_results_and_the_tools_account_of_them() {
        let event = parse(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_01","content":"fn main() {}"}]},"tool_use_result":{"status":"completed"}}"#,
        );
        let StreamEvent::User {
            message: Some(message),
            tool_use_result,
        } = event
        else {
            panic!("unexpected event: {event:?}");
        };
        assert_eq!(
            message.content,
            [CliContentBlock::ToolResult {
                tool_use_id: "toolu_01".to_string(),
                content: json!("fn main() {}"),
                is_error: false,
            }]
        );
        assert_eq!(tool_use_result, Some(json!({"status": "completed"})));

        let StreamEvent::User {
            tool_use_result, ..
        } = parse(r#"{"type":"user","toolUseResult":"Error: no such file"}"#)
        else {
            panic!("expected a user event");
        };
        assert_eq!(tool_use_result, Some(json!("Error: no such file")));
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
                errors: None,
                structured_output: None,
                total_cost_usd: None,
                turns: None,
                session_id: None,
                api_milliseconds: None,
                usage: None,
                api_error_status: None,
                model_usage: None,
            }
        );
    }

    #[test]
    fn a_result_reports_each_models_context_window_by_name() {
        let event = parse(
            r#"{"type":"result","subtype":"success","modelUsage":{"claude-sonnet-4-6":{"inputTokens":9,"contextWindow":200000},"claude-fable-5-1":{"contextWindow":1000000,"maxOutputTokens":64000}}}"#,
        );
        let StreamEvent::Result {
            model_usage: Some(usage),
            ..
        } = event
        else {
            panic!("unexpected event: {event:?}");
        };
        assert_eq!(
            usage,
            BTreeMap::from([
                (
                    "claude-fable-5-1".to_string(),
                    ModelUsage::default().with_context_window(1_000_000)
                ),
                (
                    "claude-sonnet-4-6".to_string(),
                    ModelUsage::default().with_context_window(200_000)
                ),
            ])
        );
    }

    #[test]
    fn a_model_usage_of_another_shape_keeps_the_rest_of_the_result() {
        for usage in [r#"[1,2]"#, r#""wide""#, r#"{"claude-sonnet-4-6":5}"#] {
            let event = parse(&format!(
                r#"{{"type":"result","subtype":"success","result":"done","modelUsage":{usage}}}"#
            ));
            let StreamEvent::Result {
                model_usage,
                result,
                ..
            } = event
            else {
                panic!("unexpected event: {event:?}");
            };
            assert_eq!(model_usage, None, "{usage}");
            assert_eq!(result.as_deref(), Some("done"), "{usage}");
        }
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
    fn a_result_carries_the_final_answer() {
        let event = parse(
            r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":61000,"duration_api_ms":58000,"num_turns":7,"result":"- #42 fix uploads LIVE PASS\nDEPLOY_QA_VERDICT: ALL_VERIFIED","session_id":"sess-live-qa","total_cost_usd":0.42,"usage":{"input_tokens":12,"output_tokens":340}}"#,
        );
        let StreamEvent::Result {
            result, is_error, ..
        } = event
        else {
            panic!("unexpected event: {event:?}");
        };
        assert_eq!(
            result.as_deref(),
            Some("- #42 fix uploads LIVE PASS\nDEPLOY_QA_VERDICT: ALL_VERIFIED")
        );
        assert!(!is_error);
    }

    #[test]
    fn a_result_carries_its_error_flag() {
        let event = parse(
            r#"{"type":"result","subtype":"success","is_error":true,"num_turns":3,"result":"Claude AI usage limit reached|1790262000","session_id":"sess-live-qa"}"#,
        );
        let StreamEvent::Result {
            result, is_error, ..
        } = event
        else {
            panic!("unexpected event: {event:?}");
        };
        assert_eq!(
            result.as_deref(),
            Some("Claude AI usage limit reached|1790262000")
        );
        assert!(is_error);
    }

    #[test]
    fn a_failure_subtype_carries_its_errors_without_result_text() {
        let event = parse(
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"num_turns":30,"session_id":"sess-live-qa","errors":["Reached maximum number of turns (30)"]}"#,
        );
        let StreamEvent::Result {
            result,
            errors,
            is_error,
            turns,
            ..
        } = event
        else {
            panic!("unexpected event: {event:?}");
        };
        assert!(result.is_none());
        assert_eq!(
            errors,
            Some(vec![json!("Reached maximum number of turns (30)")])
        );
        assert!(is_error);
        assert_eq!(turns, Some(30));
    }

    #[test]
    fn errors_of_another_shape_are_absent_and_the_result_is_kept() {
        for line in [
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"errors":"Reached maximum number of turns (30)"}"#,
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"errors":{"message":"Reached maximum number of turns (30)"}}"#,
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"errors":null}"#,
        ] {
            let StreamEvent::Result {
                subtype, errors, ..
            } = parse(line)
            else {
                panic!("{line} was not read as a result");
            };
            assert_eq!(subtype.as_deref(), Some("error_max_turns"), "{line}");
            assert_eq!(errors, None, "{line}");
        }
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
