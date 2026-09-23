//! Reading `codex exec --json`.

mod event;
mod item;
mod reason;
mod token_counts;

use abnegate_llm::Usage;

use crate::event::AgentEvent;
use crate::parser::codex::event::Event;
use crate::parser::codex::item::Item;

const COMMAND: &str = "command_execution";
const COMPLETED: &str = "completed";
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

#[cfg(test)]
mod tests {
    use super::interpret;
    use crate::event::AgentEvent;
    use crate::stdout_parse_result::StdoutParseResult;

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

    fn interpret_all(sample: &str) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        for line in sample.lines() {
            interpret(line, &mut events);
        }
        events
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
