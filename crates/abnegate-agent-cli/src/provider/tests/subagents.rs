//! What a tool printed and news of a subagent reach a stream's reader as
//! they are read, scrubbed of the run's secrets, and are never folded into
//! what the run amounts to nor counted toward its output limit.

use std::time::Duration;

use abnegate_llm::Credential;
use abnegate_llm::Message;
use serde_json::json;
use tempfile::TempDir;

use super::collected;
use super::execute;
use super::replayed;
use super::replaying;
use super::settings;
use super::streamed;
use crate::event::AgentEvent;
use crate::kind::AgentKind;
use crate::output::Output;
use crate::provider::CliProvider;
use crate::subagent::Subagent;
use crate::subagent::SubagentStatus;

const SUBAGENTS_STREAM: &str = include_str!("../../../tests/fixtures/claude/subagents.jsonl");
const COLLAB_STREAM: &str = include_str!("../../../tests/fixtures/codex/collab-spawn-wait.jsonl");

const SECRET: &str = "correct-horse-battery";
const PLACEHOLDER: &str = "@SECRET@";

fn outputs(events: &[AgentEvent]) -> Vec<&Output> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Output(output) => Some(output),
            _ => None,
        })
        .collect()
}

fn subagents(events: &[AgentEvent]) -> Vec<&Subagent> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Subagent(subagent) => Some(subagent),
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

/// The subagent news `agent`'s parser reads from `stream` on its own.
fn parsed(agent: AgentKind, stream: &str) -> Vec<Subagent> {
    let mut turn = agent.turn();
    let mut events = Vec::new();
    for line in stream.lines() {
        turn.interpret(line, &mut events);
    }
    events
        .into_iter()
        .filter_map(|event| match event {
            AgentEvent::Subagent(subagent) => Some(subagent),
            _ => None,
        })
        .collect()
}

/// What a tool prints can be far longer than the agent's prose, and is the
/// tool's, not the agent's, so a run whose prose is within its limit is not
/// stopped for it.
#[tokio::test]
async fn what_a_tool_printed_is_streamed_and_never_counted_toward_the_output_limit() {
    let printed = "x".repeat(4 * 1024);
    let lines = [
        json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_01", "name": "Bash", "input": {"command": "cat big.log"}}]}, "parent_tool_use_id": null}),
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_01", "content": printed}]}}),
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Read it."}]}, "parent_tool_use_id": null}),
        json!({"type": "result", "subtype": "success", "is_error": false}),
    ]
    .map(|line| line.to_string());

    let directory = TempDir::new().expect("a temporary directory");
    let settings = settings(&directory, &replaying(&directory, &lines)).with_output_limit(64);
    let provider = CliProvider::agent(AgentKind::Claude, settings);

    let (events, failure, _) = collected(streamed(&provider, &[Message::user("hi")]).await).await;

    assert!(failure.is_none(), "{failure:?}");
    let outputs = outputs(&events);
    assert_eq!(outputs.len(), 1, "{events:?}");
    assert_eq!(outputs[0].call.function.name, "Bash");
    assert_eq!(outputs[0].text, printed);
    assert!(outputs[0].succeeded);

    let execution = execute(&provider, &[Message::user("hi")]).await;
    assert!(
        execution.stdout.failure.is_none(),
        "{:?}",
        execution.stdout.failure
    );
    assert_eq!(execution.stdout.text, "Read it.");
    assert_eq!(execution.stdout.tools.len(), 1);
}

/// A stream nobody keeps up with holds every event it has not yielded, so
/// what one tool printed is cut to the run's tool output limit, keeping the
/// start and the end a caller reads its outcome by. The text is scrubbed
/// before it is cut, so a cut through a secret leaves none of it behind.
#[tokio::test]
async fn what_a_tool_printed_past_its_limit_is_cut_after_it_is_scrubbed() {
    let link = "https://example.com/acme/widgets/pull/7";
    let printed = format!(
        "{}{PLACEHOLDER}{}{link}",
        "x".repeat(90),
        "y".repeat(4 * 1024)
    );
    let lines = [
        json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_01", "name": "Bash", "input": {"command": "gh pr create"}}]}, "parent_tool_use_id": null}),
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_01", "content": printed}]}}),
        json!({"type": "result", "subtype": "success", "is_error": false}),
    ]
    .map(|line| line.to_string());
    let directory = TempDir::new().expect("a temporary directory");
    let recording = directory.path().join("recording.jsonl");
    std::fs::write(&recording, format!("{}\n", lines.join("\n"))).expect("the recording");
    let script = format!(
        "sed \"s/{PLACEHOLDER}/$DB_PASSWORD/g\" '{}'",
        recording.display()
    );
    let settings = settings(&directory, &script)
        .with_credential(Credential::key("DB_PASSWORD", SECRET))
        .with_tool_output_limit(200);
    let provider = CliProvider::agent(AgentKind::Claude, settings);

    let (events, failure, _) = collected(streamed(&provider, &[Message::user("hi")]).await).await;

    assert!(failure.is_none(), "{failure:?}");
    let outputs = outputs(&events);
    assert_eq!(outputs.len(), 1, "{events:?}");
    let text = &outputs[0].text;
    let (head, rest) = text.split_once('\n').expect("a cut text");
    let (marker, tail) = rest.split_once('\n').expect("a cut text");
    assert_eq!(head, format!("{}[REDACTED]", "x".repeat(90)));
    assert!(!text.contains("correct"), "{text}");
    assert_eq!(marker, "[4035 bytes left out]");
    assert_eq!(tail.len(), 100);
    assert!(tail.ends_with(link), "{tail}");
}

/// Every update the parser reads reaches the stream as it arrives, the
/// background subagent's news after the turn's first result among them,
/// and none of a subagent's own calls is one of the main agent's.
#[tokio::test]
async fn news_of_a_subagent_is_streamed_as_the_parser_reads_it() {
    let directory = TempDir::new().expect("a temporary directory");
    let provider = CliProvider::agent(AgentKind::Claude, replayed(&directory, SUBAGENTS_STREAM));

    let (events, failure, _) = collected(streamed(&provider, &[Message::user("hi")]).await).await;

    assert!(failure.is_none(), "{failure:?}");
    let streamed: Vec<Subagent> = subagents(&events).into_iter().cloned().collect();
    assert_eq!(streamed, parsed(AgentKind::Claude, SUBAGENTS_STREAM));
    assert_eq!(
        streamed.last(),
        Some(
            &Subagent::new("toolu_04Review")
                .with_status(SubagentStatus::Completed)
                .with_tool_uses(3)
                .with_duration(Duration::from_secs(61))
                .with_tokens(21_000)
                .with_summary("The change is sound and its tests pass.")
        )
    );
    assert_eq!(tools(&events), ["Agent", "Agent", "Bash"]);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::Finished { .. }))
            .count(),
        2,
        "the stream reads on past the first result: {events:?}"
    );

    let execution = execute(&provider, &[Message::user("hi")]).await;
    let names: Vec<&str> = execution
        .stdout
        .tools
        .iter()
        .map(|call| call.function.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["Agent", "Agent", "Bash"],
        "a subagent's calls are not the main agent's"
    );
}

#[tokio::test]
async fn codexs_collaborators_are_streamed_as_the_parser_reads_them() {
    let directory = TempDir::new().expect("a temporary directory");
    let provider = CliProvider::agent(AgentKind::Codex, replayed(&directory, COLLAB_STREAM));

    let (events, failure, _) = collected(streamed(&provider, &[Message::user("hi")]).await).await;

    assert!(failure.is_none(), "{failure:?}");
    let streamed: Vec<Subagent> = subagents(&events).into_iter().cloned().collect();
    assert_eq!(streamed, parsed(AgentKind::Codex, COLLAB_STREAM));
    assert_eq!(streamed.len(), 5, "{events:?}");
    assert!(tools(&events).is_empty(), "{events:?}");
}

/// A tool prints whatever it reads, the run's own secrets included, and a
/// subagent reports in its own words, so neither reaches the stream
/// unscrubbed.
#[tokio::test]
async fn a_secret_a_tool_printed_or_a_subagent_reported_is_scrubbed_from_the_stream() {
    let lines = [
        json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_01Agent", "name": "Agent", "input": {"description": "Check the login", "prompt": "Check it."}}]}, "parent_tool_use_id": null}),
        json!({"type": "system", "subtype": "task_started", "task_id": "t1", "tool_use_id": "toolu_01Agent", "description": format!("Check {PLACEHOLDER}"), "task_type": "local_agent"}),
        json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_02Bash", "name": "Bash", "input": {"command": format!("login --password {PLACEHOLDER}")}}]}, "parent_tool_use_id": "toolu_01Agent"}),
        json!({"type": "system", "subtype": "task_progress", "task_id": "t1", "summary": format!("Logging in with {PLACEHOLDER}"), "last_tool_name": "Bash"}),
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_02Bash", "content": format!("logged in as admin:{PLACEHOLDER}")}]}, "parent_tool_use_id": "toolu_01Agent"}),
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_01Agent", "content": format!("The password is {PLACEHOLDER}.")}]}, "tool_use_result": {"status": "completed"}}),
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Checked."}]}, "parent_tool_use_id": null}),
        json!({"type": "result", "subtype": "success", "is_error": false}),
    ]
    .map(|line| line.to_string());
    let directory = TempDir::new().expect("a temporary directory");
    let recording = directory.path().join("recording.jsonl");
    std::fs::write(&recording, format!("{}\n", lines.join("\n"))).expect("the recording");
    let script = format!(
        "sed \"s/{PLACEHOLDER}/$DB_PASSWORD/g\" '{}'",
        recording.display()
    );
    let settings =
        settings(&directory, &script).with_credential(Credential::key("DB_PASSWORD", SECRET));
    let provider = CliProvider::agent(AgentKind::Claude, settings);

    let (events, failure, _) = collected(streamed(&provider, &[Message::user("hi")]).await).await;

    assert!(failure.is_none(), "{failure:?}");
    let outputs = outputs(&events);
    assert_eq!(outputs.len(), 2, "{events:?}");
    for output in &outputs {
        assert!(!output.text.contains(SECRET), "{output:?}");
        assert!(output.text.contains("[REDACTED]"), "{output:?}");
    }
    let subagents = subagents(&events);
    let words: Vec<&str> = subagents
        .iter()
        .flat_map(|subagent| {
            [
                subagent.description.as_deref(),
                subagent.summary.as_deref(),
                subagent
                    .step
                    .as_ref()
                    .and_then(|step| step.subject.as_deref()),
            ]
        })
        .flatten()
        .collect();
    assert!(
        words.iter().all(|words| !words.contains(SECRET)),
        "{words:?}"
    );
    assert_eq!(
        words
            .iter()
            .filter(|words| words.contains("[REDACTED]"))
            .count(),
        5,
        "the task's description, the step's subject as it ran and as it returned, its progress and its report: {words:?}"
    );
}
