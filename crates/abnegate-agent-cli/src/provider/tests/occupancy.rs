//! How a Codex run learns the window its turn ran in from the rollout Codex
//! keeps of its session, and when it does not look.

use std::path::Path;
use std::time::Duration;

use abnegate_llm::Message;
use abnegate_llm::ProviderError;
use futures::StreamExt;
use tempfile::TempDir;

use super::collected;
use super::request;
use super::settings;
use crate::event::AgentEvent;
use crate::kind::AgentKind;
use crate::occupancy::Occupancy;
use crate::provider::CliProvider;
use crate::rollout::read;
use crate::settings::CliSettings;
use crate::test_support::PATIENCE;

const ROLLOUT: &str = include_str!("../../../tests/fixtures/codex/rollout.jsonl");
const CATALOGUE: &str = include_str!("../../../tests/fixtures/codex/models_cache.json");
const THREAD: &str = "019b2c41-0000-7000-8000-000000000001";
const MODEL: &str = "gpt-6.1-sol";

/// A Codex turn on [`THREAD`] that answers and exits.
const ANSWERED: &str = r#"echo '{"type":"thread.started","thread_id":"019b2c41-0000-7000-8000-000000000001"}'
echo '{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"OK"}}'
echo '{"type":"turn.completed","usage":{"input_tokens":24633,"output_tokens":5}}'
"#;

/// A Codex turn on [`THREAD`] that fails and exits.
const FAILED: &str = r#"echo '{"type":"thread.started","thread_id":"019b2c41-0000-7000-8000-000000000001"}'
echo '{"type":"turn.failed","error":{"message":"the sandbox refused the command"}}'
exit 1
"#;

fn reported() -> Option<Occupancy> {
    Occupancy::reported(475_000, Some(24_638))
}

/// A Codex home holding `rollout` as [`THREAD`]'s rollout, and `catalogue`
/// as its models cache, when given.
fn home(rollout: Option<&str>, catalogue: Option<&str>) -> TempDir {
    let home = TempDir::new().expect("a codex home");
    if let Some(rollout) = rollout {
        let day = home.path().join("sessions/2026/11/05");
        std::fs::create_dir_all(&day).expect("a day folder");
        std::fs::write(
            day.join(format!("rollout-2026-11-05T13-36-38-{THREAD}.jsonl")),
            rollout,
        )
        .expect("the rollout");
    }
    if let Some(catalogue) = catalogue {
        std::fs::write(home.path().join("models_cache.json"), catalogue).expect("the catalogue");
    }
    home
}

fn codex(directory: &TempDir, home: &Path, script: &str) -> CliSettings {
    settings(directory, script).with_variable("CODEX_HOME", home.display().to_string())
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

#[tokio::test]
async fn a_codex_run_reports_its_rollouts_window_last_in_its_stream_and_on_its_stdout() {
    let directory = TempDir::new().expect("a temporary directory");
    let home = home(Some(ROLLOUT), None);
    let provider = CliProvider::agent(AgentKind::Codex, codex(&directory, home.path(), ANSWERED));

    let messages = [Message::user("hi")];
    let stream = provider.stream(request(&messages)).await.expect("a stream");
    let (events, failure, _) = collected(stream).await;

    assert!(failure.is_none(), "{failure:?}");
    assert!(
        matches!(
            &events[events.len() - 2..],
            [AgentEvent::Finished { .. }, AgentEvent::Occupancy(occupancy)] if Some(*occupancy) == reported()
        ),
        "{events:?}"
    );

    let execution = provider
        .execute(request(&messages), "test-run")
        .await
        .expect("an execution");
    assert_eq!(execution.stdout.occupancy, reported());
}

#[tokio::test]
async fn a_failed_codex_run_still_reports_its_window_before_its_error() {
    let directory = TempDir::new().expect("a temporary directory");
    let home = home(Some(ROLLOUT), None);
    let provider = CliProvider::agent(AgentKind::Codex, codex(&directory, home.path(), FAILED));

    let stream = provider
        .stream(request(&[Message::user("hi")]))
        .await
        .expect("a stream");
    let (events, failure, after) = collected(stream).await;

    assert!(
        matches!(failure, Some(ProviderError::Agent { .. })),
        "{failure:?}"
    );
    assert_eq!(after, 0);
    assert_eq!(occupancies(&events), [reported().expect("a window")]);
    assert!(
        matches!(events.last(), Some(AgentEvent::Occupancy(_))),
        "{events:?}"
    );
}

#[tokio::test]
async fn a_codex_rollout_without_a_report_falls_back_to_the_models_published_window() {
    let directory = TempDir::new().expect("a temporary directory");
    let unreported: String = ROLLOUT
        .lines()
        .filter(|line| !line.contains("token_count"))
        .map(|line| format!("{line}\n"))
        .collect();
    let home = home(Some(&unreported), Some(CATALOGUE));
    let provider = CliProvider::agent(AgentKind::Codex, codex(&directory, home.path(), ANSWERED));

    let execution = provider
        .execute_prompt(Some(MODEL), "hi", "test-run")
        .await
        .expect("an execution");

    assert_eq!(
        execution.stdout.occupancy,
        Occupancy::reported(500_000 * 95 / 100, None)
    );
    assert_eq!(
        execution.stdout.occupancy.map(|occupancy| occupancy.window),
        Some(475_000)
    );
}

#[tokio::test]
async fn a_codex_run_with_neither_a_report_nor_a_published_window_reports_none() {
    let directory = TempDir::new().expect("a temporary directory");
    let home = home(None, None);
    let provider = CliProvider::agent(AgentKind::Codex, codex(&directory, home.path(), ANSWERED));

    let stream = provider
        .stream_prompt(Some(MODEL), "hi")
        .await
        .expect("a stream");
    let (events, failure, _) = collected(stream).await;

    assert!(failure.is_none(), "{failure:?}");
    assert!(occupancies(&events).is_empty(), "{events:?}");
    assert!(
        read::under(home.path()),
        "the run never looked for its rollout"
    );
}

#[tokio::test]
async fn a_codex_run_that_names_no_session_never_looks_for_its_rollout() {
    let directory = TempDir::new().expect("a temporary directory");
    let home = home(Some(ROLLOUT), None);
    let script = r#"echo '{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"OK"}}'
echo '{"type":"turn.completed"}'
"#;
    let provider = CliProvider::agent(AgentKind::Codex, codex(&directory, home.path(), script));

    let execution = provider
        .execute(request(&[Message::user("hi")]), "test-run")
        .await
        .expect("an execution");

    assert!(execution.stdout.occupancy.is_none());
    assert!(!read::under(home.path()));
}

#[tokio::test]
async fn a_codex_run_that_timed_out_never_reads_its_rollout() {
    let directory = TempDir::new().expect("a temporary directory");
    let home = home(Some(ROLLOUT), None);
    let script = r#"echo '{"type":"thread.started","thread_id":"019b2c41-0000-7000-8000-000000000001"}'
exec sleep 300
"#;
    let settings = codex(&directory, home.path(), script).with_timeout(Duration::from_secs(1));
    let provider = CliProvider::agent(AgentKind::Codex, settings);

    let stream = provider
        .stream(request(&[Message::user("hi")]))
        .await
        .expect("a stream");
    let (events, failure, _) = tokio::time::timeout(PATIENCE, collected(stream))
        .await
        .expect("the run ends within its patience");

    assert!(
        matches!(failure, Some(ProviderError::Timeout { .. })),
        "{failure:?}"
    );
    assert!(occupancies(&events).is_empty(), "{events:?}");
    assert!(
        !read::under(home.path()),
        "a timed out run read its rollout"
    );
}

#[tokio::test]
async fn a_codex_run_a_tripwire_settled_never_reads_its_rollout() {
    let directory = TempDir::new().expect("a temporary directory");
    let home = home(Some(ROLLOUT), None);
    let script = r#"echo '{"type":"thread.started","thread_id":"019b2c41-0000-7000-8000-000000000001"}'
echo 'API Error: 429 Too Many Requests' >&2
sleep 1
echo '{"type":"turn.completed"}'
"#;
    let settings =
        codex(&directory, home.path(), script).with_tripwire(|line| line.contains("429"));
    let provider = CliProvider::agent(AgentKind::Codex, settings);

    let execution = provider
        .execute(request(&[Message::user("hi")]), "test-run")
        .await
        .expect("an execution");

    assert!(
        execution
            .failure
            .as_deref()
            .is_some_and(|failure| failure.contains("429")),
        "{:?}",
        execution.failure
    );
    assert!(execution.stdout.occupancy.is_none());
    assert!(!read::under(home.path()), "a tripped run read its rollout");
}

#[tokio::test]
async fn a_dropped_codex_stream_never_reads_its_rollout() {
    let directory = TempDir::new().expect("a temporary directory");
    let home = home(Some(ROLLOUT), None);
    let script = r#"echo '{"type":"thread.started","thread_id":"019b2c41-0000-7000-8000-000000000001"}'
sleep 1
echo '{"type":"turn.completed"}'
"#;
    let provider = CliProvider::agent(AgentKind::Codex, codex(&directory, home.path(), script));

    let mut stream = provider
        .stream(request(&[Message::user("hi")]))
        .await
        .expect("a stream");
    let first = tokio::time::timeout(PATIENCE, stream.next())
        .await
        .expect("the session within its patience");
    assert!(
        matches!(first, Some(Ok(AgentEvent::Session(_)))),
        "{first:?}"
    );
    drop(stream);
    tokio::time::sleep(Duration::from_secs(3)).await;

    assert!(!read::under(home.path()), "a dropped run read its rollout");
}

#[tokio::test]
async fn a_claude_run_never_looks_for_a_codex_rollout() {
    let directory = TempDir::new().expect("a temporary directory");
    let home = home(Some(ROLLOUT), None);
    let script = r#"echo '{"type":"system","subtype":"init","session_id":"019b2c41-0000-7000-8000-000000000001"}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"OK"}'
"#;
    let provider = CliProvider::agent(AgentKind::Claude, codex(&directory, home.path(), script));

    let execution = provider
        .execute(request(&[Message::user("hi")]), "test-run")
        .await
        .expect("an execution");

    assert!(execution.stdout.occupancy.is_none());
    assert!(!read::under(home.path()));
}
