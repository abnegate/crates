//! Reading `codex exec --json`.

mod event;
mod item;
mod reason;
mod token_counts;

use abnegate_llm::Usage;
use abnegate_secret::redact;
use serde_json::Map;
use serde_json::Value;

use crate::event::AgentEvent;
use crate::mcp::qualified;
use crate::parser;
use crate::parser::codex::event::Event;
use crate::parser::codex::item::Item;

const COMMAND: &str = "command_execution";
const COMPLETED: &str = "completed";
const ENDS_TURN: [&str; 2] = ["turn.completed", "turn.failed"];
const COMPLETED_ITEM: &str = "item.completed";
const MESSAGE: &str = "agent_message";
const FAILED_TURN: &str = "the agent reported a failed turn";
const ERROR: &str = "the agent reported an error";

/// Translate one line of the stream, appending whatever it means.
pub fn interpret(line: &str, events: &mut Vec<AgentEvent>) {
    let Ok(event) = serde_json::from_str::<Event>(line.trim()) else {
        return;
    };

    match event {
        Event::Thread { thread_id } => events.extend(thread_id.map(AgentEvent::Session)),
        Event::Completed {
            item: Item::Message { text },
        } => events.push(AgentEvent::Text(text)),
        Event::Completed {
            item: Item::Command { id, command },
        } => events.push(AgentEvent::tool(
            id,
            COMMAND.to_string(),
            serde_json::json!({ "command": command }).to_string(),
        )),
        Event::Completed {
            item:
                Item::Call {
                    id,
                    server,
                    tool,
                    arguments,
                    error,
                },
        } => {
            if let Some(reason) = error.and_then(|error| error.message) {
                tracing::warn!(
                    %server,
                    %tool,
                    reason = %redact(&reason),
                    "codex refused an MCP tool call"
                );
            }
            let arguments = arguments.unwrap_or_else(|| Value::Object(Map::new()));
            events.push(AgentEvent::tool(
                id,
                qualified(&server, &tool),
                arguments.to_string(),
            ));
        }
        Event::Turn { usage } => {
            events.extend(usage.map(|usage| AgentEvent::Usage(Usage::from(usage))));
            events.push(AgentEvent::Finished {
                finish_reason: Some(COMPLETED.to_string()),
            });
        }
        Event::Failed { error, message } => events.push(AgentEvent::Failed(
            error
                .and_then(|error| error.message)
                .or(message)
                .unwrap_or_else(|| FAILED_TURN.to_string()),
        )),
        Event::Error { message } => events.push(AgentEvent::Diagnostic(
            message.unwrap_or_else(|| ERROR.to_string()),
        )),
        Event::Completed {
            item: Item::Ignored,
        }
        | Event::Ignored => {}
    }
}

/// Whether an event too long to read, of which only `prefix` is known, is
/// one the run cannot do without: the end of the turn, or the agent's prose,
/// rather than a command's output or other event that can be dropped.
pub fn essential(prefix: &str) -> bool {
    let types = parser::types(prefix);
    let top = |kinds: &[&str]| {
        types
            .iter()
            .any(|(depth, kind)| *depth == 1 && kinds.contains(kind))
    };
    top(&ENDS_TURN)
        || (top(&[COMPLETED_ITEM])
            && types
                .iter()
                .any(|(depth, kind)| *depth == 2 && *kind == MESSAGE))
}

#[cfg(test)]
mod tests {
    use super::essential;
    use super::interpret;
    use crate::event::AgentEvent;
    use crate::stdout_parse_result::StdoutParseResult;
    use crate::test_support::captured_logs;

    /// Recorded from `codex exec --json --skip-git-repo-check -`.
    const SESSION: &str = r#"{"type":"thread.started","thread_id":"019b2c41-0000-7000-8000-000000000001"}
{"type":"turn.started"}
{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"cargo test","aggregated_output":"","exit_code":null,"status":"in_progress"}}
{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"cargo test","aggregated_output":"test result: ok. 12 passed\n","exit_code":0,"status":"completed"}}
{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"The suite passes."}}
{"type":"turn.completed","usage":{"input_tokens":4310,"cached_input_tokens":3900,"output_tokens":128}}"#;

    /// Recorded from a run whose stream dropped and reconnected mid-turn.
    const RECONNECTED: &str = r#"{"type":"thread.started","thread_id":"019b2c41-0000-7000-8000-000000000003"}
{"type":"turn.started"}
{"type":"error","message":"Reconnecting... 1/5 (stream disconnected before completion: error sending request for url (https://chatgpt.com/backend-api/codex/responses))"}
{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"The suite passes."}}
{"type":"turn.completed","usage":{"input_tokens":2210,"cached_input_tokens":0,"output_tokens":41}}"#;

    /// Recorded from a run that ran out of quota.
    const QUOTA: &str = r#"{"type":"thread.started","thread_id":"019b2c41-0000-7000-8000-000000000002"}
{"type":"turn.started"}
{"type":"error","message":"You have hit your usage limit. Try again later."}
{"type":"turn.failed","error":{"message":"You have hit your usage limit. Try again later."}}"#;

    /// Codex calling a relay's MCP tool, driven by a scripted Responses
    /// provider.
    const TOOL_CALL: &str = include_str!("../../tests/fixtures/codex/tool-call.jsonl");

    /// A call to an echo tool, then to a tool that takes no arguments.
    const TWO_CALLS: &str = include_str!("../../tests/fixtures/codex/two-tool-calls.jsonl");

    /// The provider dropped its first stream and codex reconnected.
    const RECONNECTED_THEN_CALLED: &str =
        include_str!("../../tests/fixtures/codex/reconnect-then-complete.jsonl");

    /// Run without the approval key, so codex refused the call itself.
    const REFUSED_BY_CODEX: &str =
        include_str!("../../tests/fixtures/codex/tool-call-without-approval.jsonl");

    /// The relay refused the call, as it does when the reader denies one.
    const REFUSED_BY_SERVER: &str =
        include_str!("../../tests/fixtures/codex/tool-call-denied-by-server.jsonl");

    /// No sign-in, against the real API, retried until codex gave up.
    const SIGNED_OUT: &str = include_str!("../../tests/fixtures/codex/unauthenticated.jsonl");

    /// A real model, qwen2.5:7b-instruct on Ollama.
    const LOCAL_MODEL: &str =
        include_str!("../../tests/fixtures/codex/local-model-tool-call.jsonl");

    /// The call is made from inside code mode's `exec`.
    const CODE_MODE: &str = include_str!("../../tests/fixtures/codex/code-mode-tool-call.jsonl");

    fn interpret_all(sample: &str) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        for line in sample.lines() {
            interpret(line, &mut events);
        }
        events
    }

    fn calls(events: &[AgentEvent]) -> Vec<(&str, &str, &str)> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Tool(call) => Some((
                    call.id.as_str(),
                    call.function.name.as_str(),
                    call.function.arguments.as_str(),
                )),
                _ => None,
            })
            .collect()
    }

    fn answer(events: &[AgentEvent]) -> String {
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

    fn finished(events: &[AgentEvent]) -> bool {
        matches!(events.last(), Some(AgentEvent::Finished { .. }))
    }

    #[test]
    fn a_call_to_an_mcp_tool_is_reported_under_the_name_the_model_saw() {
        let events = interpret_all(TOOL_CALL);

        assert_eq!(
            calls(&events),
            [(
                "item_0",
                "mcp__relay__echo",
                r#"{"text":"r6-rung3-nonce-9b2d"}"#
            )]
        );
        assert_eq!(
            answer(&events),
            "The echo tool returned: Wall time: 0.0035 seconds\nOutput: r6-rung3-nonce-9b2d"
        );
        assert!(failures(&events).is_empty(), "{:?}", failures(&events));
        assert!(finished(&events), "{events:?}");
    }

    #[test]
    fn every_call_is_reported_including_one_that_takes_no_arguments() {
        let events = interpret_all(TWO_CALLS);

        assert_eq!(
            calls(&events),
            [
                (
                    "item_0",
                    "mcp__relay__echo",
                    r#"{"text":"r6-rung3-nonce-9b2d"}"#
                ),
                ("item_1", "mcp__relay__memory_list", "{}"),
            ]
        );
        assert!(finished(&events), "{events:?}");
    }

    #[test]
    fn a_call_without_arguments_at_all_is_reported_with_empty_ones() {
        let mut events = Vec::new();
        interpret(
            r#"{"type":"item.completed","item":{"id":"item_0","type":"mcp_tool_call","server":"relay","tool":"memory_list","status":"completed"}}"#,
            &mut events,
        );

        assert_eq!(
            calls(&events),
            [("item_0", "mcp__relay__memory_list", "{}")]
        );
    }

    #[test]
    fn a_call_codex_refused_itself_is_reported_and_the_turn_goes_on() {
        let (events, logs) = captured_logs(|| interpret_all(REFUSED_BY_CODEX));

        assert_eq!(
            calls(&events),
            [(
                "item_0",
                "mcp__relay__echo",
                r#"{"text":"r6-rung3-nonce-9b2d"}"#
            )]
        );
        assert!(
            failures(&events).is_empty(),
            "a refused call is the model's to answer, not the end of the turn: {:?}",
            failures(&events)
        );
        assert!(finished(&events), "{events:?}");
        assert!(
            logs.contains("WARN")
                && logs.contains("MCP tool call requires approval, but approval policy is never"),
            "the refusal the server never saw went unlogged: {logs}"
        );
    }

    #[test]
    fn a_call_the_server_refused_is_reported_and_the_turn_goes_on() {
        let events = interpret_all(REFUSED_BY_SERVER);

        assert_eq!(
            calls(&events),
            [(
                "item_0",
                "mcp__relay__echo",
                r#"{"text":"deny: r6-rung3-nonce-9b2d"}"#
            )]
        );
        assert_eq!(
            answer(&events),
            "The relay refused the call: Wall time: 0.0014 seconds\nOutput: The user denied this tool call."
        );
        assert!(failures(&events).is_empty(), "{:?}", failures(&events));
        assert!(finished(&events), "{events:?}");
    }

    #[test]
    fn an_error_codex_recovers_from_still_ends_in_its_answer() {
        let events = interpret_all(RECONNECTED_THEN_CALLED);

        assert!(
            failures(&events).is_empty(),
            "a reconnect codex went on to recover from failed the turn: {:?}",
            failures(&events)
        );
        assert_eq!(
            answer(&events),
            "The echo tool returned: Wall time: 0.0012 seconds\nOutput: r6-rung3-nonce-9b2d"
        );
        assert!(finished(&events), "{events:?}");

        let mut result: StdoutParseResult = events.into_iter().collect();
        result.conclude();
        assert!(result.failure.is_none(), "{:?}", result.failure);
    }

    #[test]
    fn an_error_line_on_its_own_ends_nothing() {
        let mut events = Vec::new();
        interpret(
            r#"{"type":"error","message":"Reconnecting... 1/5 (stream disconnected before completion: stream closed before response.completed)"}"#,
            &mut events,
        );

        assert!(
            matches!(events.as_slice(), [AgentEvent::Diagnostic(_)]),
            "{events:?}"
        );
        assert!(!events.iter().any(AgentEvent::terminal), "{events:?}");
    }

    #[test]
    fn a_turn_that_fails_after_every_retry_fails_once_in_codexs_own_words() {
        let events = interpret_all(SIGNED_OUT);

        let failures = failures(&events);
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(
            failures[0].contains("401 Unauthorized: Missing bearer or basic authentication"),
            "{failures:?}"
        );
        assert!(!finished(&events), "{events:?}");
    }

    #[test]
    fn a_real_models_call_reads_like_a_scripted_one() {
        let events = interpret_all(LOCAL_MODEL);

        assert_eq!(
            calls(&events),
            [(
                "item_1",
                "mcp__relay__echo",
                r#"{"text":"r6-rung2-nonce-4c1e"}"#
            )]
        );
        assert_eq!(
            answer(&events),
            "The output of the echo tool is `r6-rung2-nonce-4c1e`."
        );
        assert!(failures(&events).is_empty(), "{:?}", failures(&events));
        assert!(finished(&events), "{events:?}");
    }

    #[test]
    fn a_call_made_from_code_mode_reads_like_a_direct_one() {
        let events = interpret_all(CODE_MODE);

        assert_eq!(
            calls(&events),
            [(
                "item_0",
                "mcp__relay__echo",
                r#"{"text":"t6-a22-nonce-5e1f"}"#
            )]
        );
        assert!(failures(&events).is_empty(), "{:?}", failures(&events));
        assert!(finished(&events), "{events:?}");
    }

    #[test]
    fn a_recorded_session_yields_its_text_command_and_completion() {
        let events = interpret_all(SESSION);

        let text: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, ["The suite passes."]);

        let tools: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Tool(call) => Some(call),
                _ => None,
            })
            .collect();
        assert_eq!(tools.len(), 1, "one completed command, not its start too");
        assert_eq!(tools[0].id, "item_1");
        assert_eq!(tools[0].function.name, "command_execution");
        assert_eq!(tools[0].function.arguments, r#"{"command":"cargo test"}"#);

        assert!(matches!(events.last(), Some(AgentEvent::Finished { .. })));
    }

    #[test]
    fn the_thread_identifier_is_reported_as_the_session() {
        let events = interpret_all(SESSION);

        assert!(matches!(
            events.first(),
            Some(AgentEvent::Session(session)) if session == "019b2c41-0000-7000-8000-000000000001"
        ));
    }

    #[test]
    fn the_completed_turn_carries_the_token_counts() {
        let events = interpret_all(SESSION);

        let usage = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::Usage(usage) => Some(usage),
                _ => None,
            })
            .expect("a usage event");

        assert_eq!(usage.prompt_tokens, 4310);
        assert_eq!(usage.completion_tokens, 128);
        assert_eq!(usage.total_tokens, 4438);
    }

    #[test]
    fn an_exhausted_quota_reads_as_a_rate_limit_to_the_caller() {
        let events = interpret_all(QUOTA);

        let [
            AgentEvent::Session(_),
            AgentEvent::Diagnostic(diagnostic),
            AgentEvent::Failed(failure),
        ] = events.as_slice()
        else {
            panic!("expected a diagnostic then the failed turn, got {events:?}");
        };
        for message in [diagnostic, failure] {
            assert!(
                message.to_ascii_lowercase().contains("usage limit"),
                "the caller cannot classify {message:?}"
            );
        }
    }

    #[test]
    fn a_reconnect_notice_does_not_end_a_turn_that_goes_on_to_complete() {
        let events = interpret_all(RECONNECTED);

        let terminal: Vec<&AgentEvent> = events.iter().filter(|event| event.terminal()).collect();
        assert!(
            matches!(terminal.as_slice(), [AgentEvent::Finished { .. }]),
            "{events:?}"
        );

        let mut result: StdoutParseResult = events.into_iter().collect();
        result.conclude();
        assert!(result.failure.is_none(), "{:?}", result.failure);
        assert!(result.finished);
        assert_eq!(result.text, "The suite passes.");
        assert!(
            result
                .diagnostic
                .as_deref()
                .is_some_and(|diagnostic| diagnostic.starts_with("Reconnecting"))
        );
    }

    #[test]
    fn an_error_with_no_turn_after_it_becomes_the_failure() {
        let mut result: StdoutParseResult = interpret_all(
            r#"{"type":"turn.started"}
{"type":"error","message":"stream disconnected before completion"}"#,
        )
        .into_iter()
        .collect();
        result.conclude();

        assert_eq!(
            result.failure.as_deref(),
            Some("stream disconnected before completion")
        );
    }

    #[test]
    fn a_failed_turn_without_a_nested_reason_uses_its_own_message() {
        let mut events = Vec::new();
        interpret(
            r#"{"type":"turn.failed","message":"stream disconnected"}"#,
            &mut events,
        );

        let [AgentEvent::Failed(message)] = events.as_slice() else {
            panic!("expected one failure, got {events:?}");
        };
        assert_eq!(message, "stream disconnected");
    }

    #[test]
    fn a_failed_turn_or_error_with_no_wording_at_all_still_reports_one() {
        let mut events = Vec::new();
        interpret(r#"{"type":"turn.failed"}"#, &mut events);
        interpret(r#"{"type":"error"}"#, &mut events);

        assert!(matches!(
            events.as_slice(),
            [AgentEvent::Failed(turn), AgentEvent::Diagnostic(error)]
                if turn == "the agent reported a failed turn" && error == "the agent reported an error"
        ));
    }

    #[test]
    fn a_turn_without_usage_still_finishes() {
        let mut events = Vec::new();
        interpret(r#"{"type":"turn.completed"}"#, &mut events);

        assert!(matches!(
            events.as_slice(),
            [AgentEvent::Finished { finish_reason }] if finish_reason.as_deref() == Some("completed")
        ));
    }

    #[test]
    fn a_started_item_is_not_mistaken_for_a_finished_one() {
        let mut events = Vec::new();
        interpret(
            r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"ls"}}"#,
            &mut events,
        );
        assert!(events.is_empty(), "a start produced {events:?}");
    }

    #[test]
    fn only_prose_or_the_end_of_a_turn_is_essential_when_too_long_to_read() {
        for prefix in [
            r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"#,
            r#"{"type":"turn.completed","usage":{"#,
            r#"{"type":"turn.failed","error":{"message":"#,
        ] {
            assert!(essential(prefix), "{prefix}");
        }
        for prefix in [
            r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"cat big.log","aggregated_output":"#,
            r#"{"type":"item.started","item":{"id":"item_2","type":"agent_message","text":"#,
            r#"{"type":"error","message":"#,
        ] {
            assert!(!essential(prefix), "{prefix}");
        }
    }

    #[test]
    fn unknown_events_and_noise_are_skipped() {
        let mut events = Vec::new();
        for line in [
            r#"{"type":"thread.started"}"#,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"item.completed","item":{"type":"invented_next_release"}}"#,
            r#"{"type":"invented_next_release"}"#,
            "[not json",
            "",
        ] {
            interpret(line, &mut events);
        }
        assert!(events.is_empty(), "noise produced {events:?}");
    }
}
