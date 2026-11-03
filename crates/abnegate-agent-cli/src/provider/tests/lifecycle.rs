//! How a run waits on its agent, cleans up after it and reaps it, and how it
//! answers to its marker and its registry.

use std::path::Path;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use abnegate_exec::GroupRegistry;
use abnegate_exec::executor::GRACE_PERIOD;
use abnegate_llm::Completion;
use abnegate_llm::ExitStatus;
use abnegate_llm::Message;
use abnegate_llm::ProviderError;
use tempfile::TempDir;

use super::reach;
use super::request;
use super::run;
use super::settings;
use super::timed;
use super::written_pid;
use crate::cleanup::Cleanup;
use crate::kind::AgentKind;
use crate::provider::CliProvider;
use crate::settings::CliSettings;
use crate::test_support::INTERRUPTED;
use crate::test_support::MARKER;
use crate::test_support::PATIENCE;
use crate::test_support::Straggler;
use crate::test_support::TERMINATION;
use crate::test_support::exits_within;
use crate::test_support::stop_if_running;

const ANSWER: &str = "Done.";

/// Shorter than the [`GRACE_PERIOD`] a sweep gives the processes it
/// terminates, so a sweep that waits it out outlasts the timeout.
const SHORT_TIMEOUT: Duration = Duration::from_secs(3);

/// The stream of a turn that answers [`ANSWER`].
fn answering() -> String {
    format!(
        r#"echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{ANSWER}"}}]}}}}'
echo '{{"type":"result","subtype":"success","is_error":false}}'
"#
    )
}

/// The stream of a turn a rate limit refused.
const REFUSED: &str = r#"echo '{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour"}}'
"#;

fn provider(settings: CliSettings) -> CliProvider {
    CliProvider::agent(AgentKind::Claude, settings)
}

/// A registry of the test's own, which no other test can interrupt or kill.
fn registry() -> Arc<GroupRegistry> {
    Arc::new(GroupRegistry::new())
}

/// A directory, beside the fake agent, for a [`Straggler`]'s records.
fn records(directory: &TempDir) -> PathBuf {
    let records = directory.path().join("records");
    std::fs::create_dir(&records).expect("the straggler's records");
    records
}

fn answer(completion: Result<Completion, ProviderError>) -> String {
    completion
        .expect("an answer")
        .message
        .content
        .expect("prose")
}

/// The pid the fake agent wrote to `path`.
fn pid(path: &Path) -> u32 {
    std::fs::read_to_string(path)
        .expect("a pid")
        .trim()
        .parse()
        .expect("a pid")
}

#[tokio::test]
async fn a_run_cleaned_up_after_every_exit_returns_while_a_forked_process_holds_its_output() {
    let directory = TempDir::new().expect("a temporary directory");
    let started = directory.path().join("started");
    let background = directory.path().join("background");
    let script = format!(
        "touch '{}'\nsleep 300 &\necho $! > '{}'\n{}",
        started.display(),
        background.display(),
        answering()
    );
    let provider = provider(settings(&directory, &script).with_cleanup(Cleanup::Always));
    let messages = [Message::user("hi")];

    let (completion, waited) = timed(run(&provider, &messages), &started).await;

    let background = pid(&background);
    let stopped = exits_within(background);
    stop_if_running(background);
    assert_eq!(answer(completion), ANSWER);
    assert!(
        waited < GRACE_PERIOD / 2,
        "waited {waited:?} on a forked process the run kills as its agent exits"
    );
    assert!(stopped, "forked process {background} outlived the run");
}

#[tokio::test]
async fn a_marked_run_stops_what_its_agent_left_in_a_session_of_its_own() {
    let directory = TempDir::new().expect("a temporary directory");
    let records = records(&directory);
    let script = format!(
        "{}{}",
        Straggler::Escapes.script(&records, true),
        answering()
    );
    let provider = provider(
        settings(&directory, &script)
            .with_marker(MARKER)
            .with_cleanup(Cleanup::Always),
    );

    let completion = tokio::time::timeout(PATIENCE, run(&provider, &[Message::user("hi")]))
        .await
        .expect("the run ends within its patience");

    let escaped = Straggler::pid(&records);
    let stopped = exits_within(escaped);
    stop_if_running(escaped);
    assert_eq!(answer(completion), ANSWER);
    assert!(
        stopped,
        "process {escaped} that left the agent's process group outlived the run"
    );
}

#[tokio::test]
async fn a_failed_marked_run_stops_what_its_agent_left_in_a_session_of_its_own() {
    let directory = TempDir::new().expect("a temporary directory");
    let records = records(&directory);
    let script = format!("{}{REFUSED}", Straggler::Escapes.script(&records, false));
    let provider = provider(settings(&directory, &script).with_marker(MARKER));

    let error = run(&provider, &[Message::user("hi")])
        .await
        .expect_err("a refused turn");

    let escaped = Straggler::pid(&records);
    let stopped = exits_within(escaped);
    stop_if_running(escaped);
    assert!(error.to_string().contains("rate limit reached"), "{error}");
    assert!(
        stopped,
        "process {escaped} that left the failed run's process group outlived it"
    );
}

#[tokio::test]
async fn a_successful_marked_run_leaves_what_its_agent_started_alone() {
    let directory = TempDir::new().expect("a temporary directory");
    let records = records(&directory);
    let script = format!(
        "{}{}",
        Straggler::Escapes.script(&records, false),
        answering()
    );
    let provider = provider(settings(&directory, &script).with_marker(MARKER));

    let completion = run(&provider, &[Message::user("hi")]).await;

    let escaped = Straggler::pid(&records);
    let running = stop_if_running(escaped);
    assert_eq!(answer(completion), ANSWER);
    assert!(running, "a successful run stopped {escaped}");
}

#[tokio::test]
async fn a_successful_run_cleaned_up_after_every_exit_leaves_nothing_its_agent_forked() {
    let directory = TempDir::new().expect("a temporary directory");
    let server = directory.path().join("server");
    let script = format!(
        "sleep 300 >/dev/null 2>&1 &\necho $! > '{}'\n{}",
        server.display(),
        answering()
    );
    let provider = provider(settings(&directory, &script).with_cleanup(Cleanup::Always));

    let completion = run(&provider, &[Message::user("hi")]).await;

    let server = pid(&server);
    let stopped = exits_within(server);
    stop_if_running(server);
    assert_eq!(answer(completion), ANSWER);
    assert!(stopped, "a run cleaned up after every exit left {server}");
}

/// The exit status is read while the agent is still unreaped, so a turn it
/// finished and then failed with a nonzero exit is cleaned up after.
#[tokio::test]
async fn a_finished_turn_that_exits_nonzero_stops_what_its_agent_forked() {
    let directory = TempDir::new().expect("a temporary directory");
    let server = directory.path().join("server");
    let script = format!(
        "sleep 300 >/dev/null 2>&1 &\necho $! > '{}'\n{}exit 3",
        server.display(),
        answering()
    );
    let provider = provider(settings(&directory, &script));

    let error = run(&provider, &[Message::user("hi")])
        .await
        .expect_err("a nonzero exit");

    let server = pid(&server);
    let stopped = exits_within(server);
    stop_if_running(server);
    assert!(matches!(error, ProviderError::Exit { .. }), "{error:?}");
    assert!(
        stopped,
        "a run whose agent exited nonzero left {server} running"
    );
}

#[tokio::test]
async fn a_run_cleans_up_while_its_agent_is_still_unreaped() {
    let directory = TempDir::new().expect("a temporary directory");
    let records = records(&directory);
    let script = format!(
        "{}{}",
        Straggler::ReportsLeader.script(&records, false),
        answering()
    );
    let provider = provider(
        settings(&directory, &script)
            .with_marker(MARKER)
            .with_cleanup(Cleanup::Always),
    );

    let completion = run(&provider, &[Message::user("hi")]).await;

    let escaped = Straggler::pid(&records);
    stop_if_running(escaped);
    let leader = Straggler::leader(&records);
    assert_eq!(answer(completion), ANSWER);
    assert!(
        leader.starts_with('Z'),
        "the agent was {leader:?} rather than an unreaped zombie while the run cleaned up"
    );
}

#[tokio::test]
async fn a_run_finishes_its_sweep_past_its_timeout() {
    let directory = TempDir::new().expect("a temporary directory");
    let records = records(&directory);
    let script = format!(
        "{}{}",
        Straggler::IgnoresTerminate.script(&records, false),
        answering()
    );
    let provider = provider(
        settings(&directory, &script)
            .with_timeout(SHORT_TIMEOUT)
            .with_marker(MARKER)
            .with_cleanup(Cleanup::Always),
    );

    let execution = tokio::time::timeout(
        PATIENCE,
        provider.execute(request(&[Message::user("hi")]), "test-run"),
    )
    .await
    .expect("the run ends within its patience");

    let escaped = Straggler::pid(&records);
    let stopped = exits_within(escaped);
    stop_if_running(escaped);
    let execution = execution.expect("an agent that exited before its timeout did not time out");
    assert_eq!(execution.stdout.text, ANSWER);
    assert_eq!(
        Straggler::terminations(&records),
        TERMINATION,
        "process {escaped} must get SIGTERM once, from a single sweep"
    );
    assert!(
        stopped,
        "process {escaped} that ignores SIGTERM outlived the sweep's grace"
    );
}

#[tokio::test]
async fn a_marked_run_whose_agent_outlives_its_timeout_times_out() {
    let directory = TempDir::new().expect("a temporary directory");
    let registry = registry();
    let provider = provider(
        settings(&directory, "exec sleep 300")
            .with_timeout(Duration::from_secs(1))
            .with_marker(MARKER)
            .with_registry(Arc::clone(&registry))
            .with_cleanup(Cleanup::Always),
    );

    let error = tokio::time::timeout(PATIENCE, run(&provider, &[Message::user("hi")]))
        .await
        .expect("the run ends within its patience")
        .expect_err("an agent that outlives its timeout times out");

    assert!(matches!(error, ProviderError::Timeout { .. }), "{error:?}");
    assert!(registry.is_empty(), "the timed out run is still registered");
}

#[tokio::test]
async fn dropping_a_run_kills_its_agent_and_everything_it_left_behind() {
    let directory = TempDir::new().expect("a temporary directory");
    let records = records(&directory);
    let pids = directory.path().join("pids");
    let registry = registry();
    let script = format!(
        "{}sleep 300 &\necho $! > '{}.part'\necho $$ > '{}'\nmv '{}.part' '{}'\nexec sleep 300",
        Straggler::Escapes.script(&records, false),
        records.join("background").display(),
        pids.display(),
        records.join("background").display(),
        records.join("background").display(),
    );
    let provider = provider(
        settings(&directory, &script)
            .with_marker(MARKER)
            .with_registry(Arc::clone(&registry)),
    );
    let background = records.join("background");

    let leader = {
        let messages = [Message::user("hi")];
        let mut execution = pin!(provider.execute(request(&messages), "test-run"));
        assert!(
            reach(execution.as_mut(), &background).await.is_none(),
            "the run ended before it could be dropped"
        );
        assert!(!registry.is_empty(), "the running run is not registered");
        written_pid(&pids).await
    };

    let background = pid(&background);
    let escaped = Straggler::pid(&records);
    let stopped = [leader, background, escaped].map(exits_within);
    for pid in [leader, background, escaped] {
        stop_if_running(pid);
    }
    assert_eq!(
        stopped, [true; 3],
        "the agent {leader}, the process it forked {background} and the one that escaped {escaped} must all stop with the dropped run"
    );
    let started = std::time::Instant::now();
    while !registry.is_empty() && started.elapsed() < PATIENCE {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(registry.is_empty(), "the dropped run is still registered");
}

#[tokio::test]
async fn a_registered_run_stays_registered_until_its_agent_is_reaped() {
    let directory = TempDir::new().expect("a temporary directory");
    let started = directory.path().join("started");
    let proceed = directory.path().join("proceed");
    let registry = registry();
    let script = format!(
        "touch '{}'\nwhile [ ! -e '{}' ]; do sleep 0.05; done\n{}",
        started.display(),
        proceed.display(),
        answering()
    );
    let provider = provider(
        settings(&directory, &script)
            .with_marker(MARKER)
            .with_registry(Arc::clone(&registry)),
    );

    let messages = [Message::user("hi")];

    let mut execution = pin!(provider.execute(request(&messages), "test-run"));
    assert!(
        reach(execution.as_mut(), &started).await.is_none(),
        "the run ended before the agent was told to proceed"
    );
    let registered = !registry.is_empty();
    std::fs::File::create(&proceed).expect("the agent is told to proceed");
    let execution = execution.await.expect("an execution");

    assert!(registered, "the running run is not registered");
    assert_eq!(execution.stdout.text, ANSWER);
    assert!(registry.is_empty(), "the finished run is still registered");
}

#[tokio::test]
async fn interrupting_the_registry_passes_a_ctrl_c_on_to_the_agent() {
    let directory = TempDir::new().expect("a temporary directory");
    let records = records(&directory);
    let registry = registry();
    let script = Straggler::Interruptible.exec(&records);
    let provider = provider(settings(&directory, &script).with_registry(Arc::clone(&registry)));

    let messages = [Message::user("hi")];

    let mut execution = pin!(provider.execute(request(&messages), "test-run"));
    assert!(
        reach(execution.as_mut(), &Straggler::announcement(&records))
            .await
            .is_none(),
        "the run ended before it was interrupted"
    );
    registry
        .interrupt_all()
        .expect("the agent's group is interrupted");
    let execution = tokio::time::timeout(PATIENCE, execution)
        .await
        .expect("the interrupted agent exits")
        .expect("an execution");

    assert_eq!(execution.status, ExitStatus::Code(INTERRUPTED));
    assert!(execution.stopped.is_none(), "{:?}", execution.stopped);
    assert!(registry.is_empty());
}

#[tokio::test]
async fn a_registry_that_was_killed_starts_no_run() {
    let directory = TempDir::new().expect("a temporary directory");
    let started = directory.path().join("started");
    let registry = registry();
    registry.kill_all().expect("an empty registry is killed");
    let script = format!("touch '{}'\n{}", started.display(), answering());
    let provider = provider(
        settings(&directory, &script)
            .with_marker(MARKER)
            .with_registry(Arc::clone(&registry)),
    );

    let failure = provider
        .execute(request(&[Message::user("hi")]), "test-run")
        .await
        .expect_err("a killed registry refuses the run");

    assert!(
        matches!(*failure.error, ProviderError::Unavailable { .. }),
        "{failure:?}"
    );
    assert!(!started.exists(), "the agent started");
    assert!(
        registry.is_empty(),
        "the refused run left its marker tracked"
    );
}

#[tokio::test]
async fn a_marker_no_environment_entry_can_carry_is_refused_before_the_agent_starts() {
    let directory = TempDir::new().expect("a temporary directory");
    let started = directory.path().join("started");
    let script = format!("touch '{}'\n{}", started.display(), answering());
    let provider = provider(settings(&directory, &script).with_marker("RUN="));

    let failure = provider
        .execute(request(&[Message::user("hi")]), "test-run")
        .await
        .expect_err("an unusable marker is refused");

    assert!(
        matches!(*failure.error, ProviderError::Config { .. }),
        "{failure:?}"
    );
    assert!(!started.exists(), "the agent started");
}
