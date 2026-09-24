use std::path::Path;

use abnegate_agent::AgentPhase;
use abnegate_agent::FileSessionStore;
use abnegate_agent::SessionStore;
use abnegate_llm::Role;
use serde_json::Value;
use tempfile::TempDir;
use uuid::Uuid;

/// Written by `FileSessionStore::save` at 5e4dd69, before this crate's
/// renames: a finished run with a summary, a round that called a tool, and an
/// answer carrying a generated image.
const WRITTEN_BEFORE_THE_RENAMES: &str = include_str!("fixtures/session_written_at_5e4dd69.json");

fn identity(fixture: &Value) -> Uuid {
    fixture["id"]
        .as_str()
        .and_then(|id| Uuid::parse_str(id).ok())
        .expect("the fixture names its session")
}

async fn saved(directory: &Path, fixture: &Value) -> FileSessionStore {
    tokio::fs::write(
        directory.join(format!("{}.json", identity(fixture))),
        WRITTEN_BEFORE_THE_RENAMES,
    )
    .await
    .expect("the fixture is copied into the store");
    FileSessionStore::new(directory.to_path_buf())
}

#[tokio::test]
async fn a_session_saved_before_the_renames_still_loads() {
    let fixture: Value = serde_json::from_str(WRITTEN_BEFORE_THE_RENAMES).unwrap();
    let directory = TempDir::new().unwrap();
    let store = saved(directory.path(), &fixture).await;

    let session = store
        .load(identity(&fixture))
        .await
        .expect("the session loads");

    assert_eq!(session.title, "Build graph");
    assert_eq!(session.project_directory.as_deref(), Some("/work/project"));
    let state = &session.state;
    assert_eq!(state.id, session.id);
    assert_eq!(state.phase, AgentPhase::Complete);
    assert!(state.finished);
    assert_eq!(state.final_response.as_deref(), Some("Here is the graph."));
    assert_eq!(
        (state.consumed, state.iteration, state.tokens_used),
        (4, 2, 321)
    );

    let summary = state.summary.as_ref().expect("the summary");
    assert_eq!(summary.content, "The user asked for the build graph.");
    assert_eq!(summary.coverage.entries, ["message-2", "message-3"]);
    assert_eq!(summary.coverage.fingerprint, "a1b2c3");
    assert_eq!(summary.revision, 2);

    let roles: Vec<Role> = state.messages.iter().map(|message| message.role).collect();
    assert_eq!(
        roles,
        [
            Role::System,
            Role::User,
            Role::Assistant,
            Role::Tool,
            Role::Assistant
        ]
    );
    assert_eq!(state.messages[1].images, ["/api/artifacts/source.png"]);
    let envelope = state.messages[2]
        .tool_calls
        .as_ref()
        .expect("the tool calls");
    assert_eq!(envelope[0].id, "call_1");
    assert_eq!(envelope[0].function.name, "read_file");
    assert_eq!(
        state.messages[2].reasoning_content.as_deref(),
        Some("Read the manifest first.")
    );
    assert_eq!(state.messages[3].tool_call_id.as_deref(), Some("call_1"));
    assert_eq!(
        state.messages[4].generated_images[0].image_url.url,
        "data:image/png;base64,iVBORw0KGgo="
    );

    assert_eq!(state.steps.len(), 2);
    let acting = &state.steps[0];
    assert_eq!(acting.phase, AgentPhase::Acting);
    let results = acting.tool_calls.as_ref().expect("the round's tool calls");
    assert_eq!(results[0].call.function.name, "read_file");
    assert_eq!(results[0].result, "[workspace]\nmembers = [\"crates/*\"]");
    assert!(results[0].success);
    let responding = &state.steps[1];
    assert_eq!(responding.phase, AgentPhase::Responding);
    assert_eq!(
        responding
            .message
            .as_ref()
            .and_then(|message| message.content.as_deref()),
        Some("Here is the graph.")
    );
}

/// Every field keeps the name it was saved under, `duration_milliseconds`
/// and `project_directory` included, so a session saved now is read back by
/// an older build as well.
#[tokio::test]
async fn a_session_saved_before_the_renames_is_saved_again_unchanged() {
    let fixture: Value = serde_json::from_str(WRITTEN_BEFORE_THE_RENAMES).unwrap();
    let directory = TempDir::new().unwrap();
    let store = saved(directory.path(), &fixture).await;

    let session = store.load(identity(&fixture)).await.unwrap();

    assert_eq!(serde_json::to_value(&session).unwrap(), fixture);
}

#[tokio::test]
async fn a_session_saved_before_the_renames_is_listed() {
    let fixture: Value = serde_json::from_str(WRITTEN_BEFORE_THE_RENAMES).unwrap();
    let directory = TempDir::new().unwrap();
    let store = saved(directory.path(), &fixture).await;

    let listed = store.list().await.unwrap();

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, identity(&fixture));
    assert_eq!(listed[0].title, "Build graph");
    assert_eq!(
        listed[0].project_directory.as_deref(),
        Some("/work/project")
    );
    assert!(listed[0].finished);
}
