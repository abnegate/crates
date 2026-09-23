use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{oneshot, watch};
use uuid::Uuid;

use super::entry::Job;
use super::jobs::JOBS;
use super::limits::Limits;
use super::*;
use crate::test_support::captured_logs;
use crate::tools::{Session, ToolContext};

const POLL: Duration = Duration::from_millis(20);
const POLL_LIMIT: usize = 500;

/// The directory the log directory sits in, which a teardown takes too when
/// the logs were the only thing in it.
fn ours(cwd: &Path) -> PathBuf {
    application_directory(cwd, &crate::Application::default())
}

fn logs(cwd: &Path) -> PathBuf {
    log_directory(cwd, &crate::Application::default())
}

fn excluded_line() -> String {
    excluded(&crate::Application::default())
}

fn job() -> JobStarted {
    JobStarted {
        id: "job_9f3c1a7b2e04".to_string(),
        pid: 48213,
        log_path: "/tmp/work/.abnegate/jobs/job_9f3c1a7b2e04.log".to_string(),
    }
}

fn task() -> Session {
    Session::Task(Uuid::new_v4())
}

fn chat() -> Session {
    Session::Chat(Uuid::new_v4())
}

fn environment() -> HashMap<String, String> {
    HashMap::from([(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    )])
}

fn directory() -> TempDir {
    TempDir::new().expect("a temporary working directory")
}

fn context(session: Session, cwd: &Path) -> ToolContext {
    ToolContext {
        working_directory: cwd.to_path_buf(),
        env: environment(),
        session,
        ..ToolContext::default()
    }
}

async fn spawned(session: Session, line: &str, cwd: &Path) -> JobStarted {
    Jobs::spawn(&JobCommand::shell(line), &context(session, cwd))
        .await
        .expect("the job starts")
}

async fn settles(session: Session, id: &str) -> JobStatus {
    for _ in 0..POLL_LIMIT {
        let tail = Jobs::read(session, id, 0, 1)
            .await
            .expect("its own session reads it");
        if tail.state.settled() {
            return tail.state;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("{id} never settled");
}

fn alive(pid: u32) -> bool {
    Process::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn git(cwd: &Path, arguments: &[&str]) {
    let status = Process::new("git")
        .args(arguments)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Agent")
        .env("GIT_AUTHOR_EMAIL", "agent@example.com")
        .env("GIT_COMMITTER_NAME", "Agent")
        .env("GIT_COMMITTER_EMAIL", "agent@example.com")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git is installed");
    assert!(status.success(), "git {arguments:?}");
}

fn repository(root: &Path) {
    git(root, &["init", "--initial-branch", "main"]);
    std::fs::write(root.join("README"), "seed").expect("the seed file is written");
    git(root, &["add", "README"]);
    git(root, &["commit", "--message", "seed"]);
}

/// A checkout shaped the way a clone made with `--template=` leaves one. An
/// empty template means git writes no `info` directory at all -- so there is
/// nothing for an append to open.
fn untemplated_repository(root: &Path) {
    git(root, &["init", "--template=", "--initial-branch", "main"]);
    std::fs::write(root.join("README"), "seed").expect("the seed file is written");
    git(root, &["add", "README"]);
    git(root, &["commit", "--message", "seed"]);
}

fn exclude_path(cwd: &Path) -> PathBuf {
    let resolved = Process::new("git")
        .args(["rev-parse", "--git-path", EXCLUDE_PATH])
        .current_dir(cwd)
        .output()
        .expect("git is installed");
    assert!(resolved.status.success(), "the fixture is a checkout");
    cwd.join(String::from_utf8_lossy(&resolved.stdout).trim())
}

fn excluded_lines(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.trim() == excluded_line())
        .count()
}

/// Every entry point refuses a detached context in the same words. Saying
/// it three ways would have a wait report a job it is not allowed to reach
/// as one that merely went away.
#[tokio::test]
async fn a_detached_context_can_neither_start_nor_read_nor_wait_on_a_job() {
    let cwd = directory();
    assert_eq!(
        Jobs::spawn(
            &JobCommand::shell("exit 0"),
            &context(Session::Detached, cwd.path())
        )
        .await,
        Err(UNAVAILABLE.to_string())
    );
    assert_eq!(
        Jobs::read(Session::Detached, "job_9f3c1a7b2e04", 0, 500).await,
        Err(UNAVAILABLE.to_string())
    );
    assert_eq!(
        Jobs::settled(Session::Detached, "job_9f3c1a7b2e04").err(),
        Some(UNAVAILABLE.to_string()),
        "a wait is the third way in and must refuse it as the other two do"
    );
    assert!(
        !logs(cwd.path()).exists(),
        "a refused spawn leaves nothing behind"
    );
}

#[tokio::test]
async fn a_job_started_by_one_session_is_invisible_to_another() {
    let cwd = directory();
    let owner = task();
    let stranger = task();
    let started = spawned(owner, "sleep 30", cwd.path()).await;

    assert_eq!(
        Jobs::read(stranger, &started.id, 0, 500).await,
        Err(missing(&started.id))
    );
    assert_eq!(
        Jobs::settled(stranger, &started.id).err(),
        Some(missing(&started.id))
    );
    assert_eq!(
        Jobs::read(Session::Chat(Uuid::new_v4()), &started.id, 0, 500).await,
        Err(missing(&started.id)),
        "a chat cannot read a task run's job either"
    );
    assert!(
        Jobs::read(owner, &started.id, 0, 500).await.is_ok(),
        "the session that started it still reads it"
    );

    Jobs::kill_session(owner).await;
    assert!(!alive(started.pid), "the test leaves no child behind");
}

#[tokio::test]
async fn a_session_teardown_leaves_no_live_child() {
    let cwd = directory();
    let session = task();
    let first = spawned(session, "sleep 30", cwd.path()).await;
    let second = spawned(session, "sleep 30", cwd.path()).await;
    let bystander = task();
    let untouched = spawned(bystander, "sleep 30", cwd.path()).await;
    assert!(alive(first.pid) && alive(second.pid), "both jobs started");

    assert_eq!(Jobs::kill_session(session).await, 2);

    assert!(!alive(first.pid), "job {} outlived its session", first.id);
    assert!(!alive(second.pid), "job {} outlived its session", second.id);
    assert!(alive(untouched.pid), "another session's job is untouched");
    assert_eq!(
        Jobs::read(session, &first.id, 0, 500).await,
        Err(missing(&first.id)),
        "a killed session's jobs are gone from the registry"
    );

    Jobs::kill_session(bystander).await;
    assert!(!alive(untouched.pid), "the test leaves no child behind");
}

#[tokio::test]
async fn a_log_past_its_ceiling_kills_the_job_and_reports_flooding() {
    let cwd = directory();
    let session = task();
    let started = Jobs::start(
        &JobCommand::shell("seq 1 20000; sleep 30"),
        &context(session, cwd.path()),
        Limits {
            log_bytes: 4 * 1024,
            log_check: POLL,
            ..Limits::default()
        },
    )
    .await
    .expect("the job starts");

    assert_eq!(settles(session, &started.id).await, JobStatus::Flooded);
    assert!(
        !alive(started.pid),
        "a flooded job is killed, not left writing"
    );
    assert_eq!(
        Jobs::settled(session, &started.id)
            .expect("the job is claimable")
            .await,
        JobExited {
            id: started.id.clone(),
            exit_code: None
        },
        "a flooded job is not a job that finished"
    );

    Jobs::kill_session(session).await;
}

#[tokio::test]
async fn a_job_outliving_its_lifetime_is_killed() {
    let cwd = directory();
    let session = task();
    let started = Jobs::start(
        &JobCommand::shell("sleep 30"),
        &context(session, cwd.path()),
        Limits {
            lifetime: Duration::from_millis(100),
            ..Limits::default()
        },
    )
    .await
    .expect("the job starts");

    assert_eq!(settles(session, &started.id).await, JobStatus::Killed);
    assert!(!alive(started.pid), "a job past its lifetime is killed");

    Jobs::kill_session(session).await;
}

#[tokio::test]
async fn a_job_that_had_already_ended_still_settles() {
    let cwd = directory();
    let session = task();
    let started = spawned(session, "exit 7", cwd.path()).await;
    assert_eq!(settles(session, &started.id).await, JobStatus::Exited(7));

    let claim = Jobs::settled(session, &started.id).expect("the job is claimable");
    let exited = tokio::time::timeout(Duration::from_secs(5), claim)
        .await
        .expect("a job that ended before the claim resolves it at once");
    assert_eq!(
        exited,
        JobExited {
            id: started.id.clone(),
            exit_code: Some(7)
        }
    );

    Jobs::kill_session(session).await;
}

#[tokio::test]
async fn three_reads_walk_the_log_with_no_gap_and_no_overlap() {
    let cwd = directory();
    let session = task();
    let started = spawned(session, "printf abcdefghij", cwd.path()).await;
    settles(session, &started.id).await;
    assert_eq!(
        started.log_path,
        log_path(cwd.path(), &crate::Application::default(), &started.id).to_string_lossy(),
        "the receipt names the log the reader opens"
    );

    let first = Jobs::read(session, &started.id, 0, 4)
        .await
        .expect("the log reads");
    let second = Jobs::read(session, &started.id, first.next, 4)
        .await
        .expect("the log reads");
    let third = Jobs::read(session, &started.id, second.next, 4)
        .await
        .expect("the log reads");
    let fourth = Jobs::read(session, &started.id, third.next, 4)
        .await
        .expect("the log reads");

    assert_eq!((first.output.as_str(), first.next), ("abcd", 4));
    assert_eq!((second.output.as_str(), second.next), ("efgh", 8));
    assert_eq!((third.output.as_str(), third.next), ("ij", 10));
    assert_eq!(
        (fourth.output.as_str(), fourth.next),
        ("", 10),
        "a cursor at the end of a finished log stays there"
    );
    assert_eq!(
        format!("{}{}{}", first.output, second.output, third.output),
        "abcdefghij"
    );

    Jobs::kill_session(session).await;
}

#[tokio::test]
async fn a_read_stops_on_a_whole_character() {
    let cwd = directory();
    let session = task();
    let started = spawned(session, "printf 'aéb'", cwd.path()).await;
    settles(session, &started.id).await;

    let first = Jobs::read(session, &started.id, 0, 2)
        .await
        .expect("the log reads");
    assert_eq!(
        (first.output.as_str(), first.next),
        ("a", 1),
        "a character the budget splits waits for the next read"
    );
    let second = Jobs::read(session, &started.id, first.next, 8)
        .await
        .expect("the log reads");
    assert_eq!((second.output.as_str(), second.next), ("éb", 4));

    Jobs::kill_session(session).await;
}

/// A log outlives the call that wrote it but not the session that could
/// read it: the registry entry it was reached through goes at teardown,
/// and a chat's logs sit in the operator's own checkout, where nothing
/// excludes them and nothing else would ever clear them.
#[tokio::test]
async fn a_session_teardown_takes_the_logs_and_the_emptied_directory_with_them() {
    let cwd = directory();
    let session = chat();
    let first = spawned(session, "printf first", cwd.path()).await;
    let second = spawned(session, "printf second", cwd.path()).await;
    settles(session, &first.id).await;
    settles(session, &second.id).await;
    let written = [&first, &second].map(|started| PathBuf::from(&started.log_path));
    for log in &written {
        assert!(log.exists(), "the job wrote no log at all: {log:?}");
    }

    assert_eq!(Jobs::kill_session(session).await, 2);

    for log in &written {
        assert!(
            !log.exists(),
            "a job log outlived the session that started it: {log:?}"
        );
    }
    assert!(
        !logs(cwd.path()).exists(),
        "the log directory outlived every log in it"
    );
    assert!(
        !ours(cwd.path()).exists(),
        "the directory the logs needed is empty and still there"
    );
}

#[tokio::test]
async fn a_teardown_leaves_a_directory_that_is_not_only_ours() {
    let cwd = directory();
    let session = chat();
    let started = spawned(session, "printf kept", cwd.path()).await;
    settles(session, &started.id).await;
    let neighbour = ours(cwd.path()).join("settings");
    std::fs::write(&neighbour, "somebody else's").expect("a neighbour is written");

    Jobs::kill_session(session).await;

    assert!(!PathBuf::from(&started.log_path).exists());
    assert!(
        !logs(cwd.path()).exists(),
        "the log directory held only logs and is ours to remove"
    );
    assert!(
        neighbour.exists(),
        "a directory holding somebody else's file is not ours to remove"
    );
}

/// The chat teardown is one point, and it holds the chat's generation
/// permit while it runs. A child wedged in uninterruptible I/O is never
/// reaped, so the wait for the reap is bounded: the kill has been sent by
/// then and only the confirmation is given up.
#[tokio::test(start_paused = true)]
async fn a_child_that_never_reports_its_end_does_not_hold_the_teardown() {
    let cwd = directory();
    let session = task();
    let id = mint();
    let log = log_path(cwd.path(), &crate::Application::default(), &id);
    tokio::fs::create_dir_all(logs(cwd.path()))
        .await
        .expect("the log directory is created");
    tokio::fs::write(&log, "wedged")
        .await
        .expect("the job wrote something before wedging");
    let (reports, state) = watch::channel(JobStatus::Running);
    let (kill, killed) = oneshot::channel();
    JOBS.insert(
        id.clone(),
        Job {
            session,
            log: log.clone(),
            state,
            kill,
        },
    );

    // The bound is inside the collector, not around it: time is paused and
    // advances whenever the runtime idles, so a wait for the collector
    // would spend the bound before the teardown had started.
    let (ended, logged) = captured_logs(tokio::time::timeout(
        KILL_TIMEOUT * 4,
        Jobs::kill_session(session),
    ))
    .await;

    assert_eq!(
        ended.expect("a teardown that cannot confirm a kill still returns"),
        1
    );
    assert!(
        logged.contains("WARN") && logged.contains(&id),
        "the lagging reap is reported at warn, naming the job: {logged}"
    );
    assert!(
        Jobs::read(session, &id, 0, 1).await.is_err(),
        "the registry entry is released whether or not the child was reaped"
    );
    assert!(!log.exists(), "and the log goes with it");
    assert!(killed.await.is_ok(), "the kill itself was still sent");
    assert_eq!(
        *reports.borrow(),
        JobStatus::Running,
        "nothing ever reported the child's end, which is the case under test"
    );
}

/// The log and the exclude write are the session's, and a directory the
/// command names moves neither.
///
/// `Path::join` neither normalises `..` nor resists an absolute argument,
/// so a directory taken from a model used to take the log tree with it —
/// and on a task run the exclude write too, into whatever repository that
/// landed in.
#[tokio::test]
async fn a_directory_the_command_names_moves_the_child_and_nothing_else() {
    let root = directory();
    let checkout = root.path().join("checkout");
    let stranger = root.path().join("stranger");
    std::fs::create_dir(&checkout).expect("the run's own checkout is created");
    std::fs::create_dir(&stranger).expect("a checkout the run does not own is created");
    repository(&checkout);
    repository(&stranger);

    let session = task();
    let started = Jobs::spawn(
        &JobCommand::shell("pwd").within(&stranger),
        &context(session, &checkout),
    )
    .await
    .expect("the job starts");
    assert_eq!(settles(session, &started.id).await, JobStatus::Exited(0));

    assert_eq!(
        started.log_path,
        log_path(&checkout, &crate::Application::default(), &started.id).to_string_lossy(),
        "the log belongs to the session's tree, whatever directory the command named"
    );
    let ran_in = Jobs::read(session, &started.id, 0, 500)
        .await
        .expect("the log reads")
        .output;
    assert_eq!(
        std::fs::canonicalize(ran_in.trim()).ok(),
        std::fs::canonicalize(&stranger).ok(),
        "the child is the one thing a named directory moves: {ran_in}"
    );
    assert!(
        !logs(&stranger).exists(),
        "a log tree was written into a checkout the session does not own"
    );
    assert_eq!(
        excluded_lines(&exclude_path(&stranger)),
        0,
        "a stranger checkout's exclude file is not the session's to append to"
    );
    assert_eq!(
        excluded_lines(&exclude_path(&checkout)),
        1,
        "the run's own checkout still keeps its job logs out of its diff"
    );

    Jobs::kill_session(session).await;
}

#[tokio::test]
async fn a_chat_job_writes_nothing_to_the_repository_exclude() {
    let cwd = directory();
    repository(cwd.path());
    let exclude = exclude_path(cwd.path());
    let before = std::fs::read_to_string(&exclude).unwrap_or_default();

    let session = chat();
    let started = spawned(session, "exit 0", cwd.path()).await;
    assert_eq!(settles(session, &started.id).await, JobStatus::Exited(0));

    assert_eq!(
        std::fs::read_to_string(&exclude).unwrap_or_default(),
        before,
        "a chat's checkout is shared, so it is not the chat's to change"
    );
    assert_eq!(excluded_lines(&exclude), 0);

    Jobs::kill_session(session).await;
}

#[tokio::test]
async fn a_task_job_excludes_its_log_directory_once_however_many_it_starts() {
    let cwd = directory();
    repository(cwd.path());
    let exclude = exclude_path(cwd.path());

    let session = task();
    let first = spawned(session, "exit 0", cwd.path()).await;
    settles(session, &first.id).await;
    assert_eq!(excluded_lines(&exclude), 1);

    let second = spawned(session, "exit 0", cwd.path()).await;
    settles(session, &second.id).await;
    assert_eq!(
        excluded_lines(&exclude),
        1,
        "a second job appends nothing the first already wrote"
    );

    Jobs::kill_session(session).await;
}

/// Every checkout a run actually works in is one of these, so this is the
/// only shape of the exclude write that production ever reaches.
#[tokio::test]
async fn a_task_job_excludes_its_logs_in_a_checkout_git_left_no_info_directory_in() {
    let cwd = directory();
    untemplated_repository(cwd.path());
    assert!(
        !cwd.path().join(".git").join("info").exists(),
        "the fixture is the shape a clone leaves behind"
    );

    let session = task();
    let started = spawned(session, "exit 0", cwd.path()).await;
    settles(session, &started.id).await;

    assert_eq!(
        excluded_lines(&exclude_path(cwd.path())),
        1,
        "a run's job logs are kept out of its diff whatever git left behind"
    );

    Jobs::kill_session(session).await;
}

#[tokio::test]
async fn a_working_directory_that_is_not_a_checkout_is_left_alone() {
    let cwd = directory();
    let session = task();
    let started = spawned(session, "exit 0", cwd.path()).await;

    assert_eq!(settles(session, &started.id).await, JobStatus::Exited(0));
    assert!(
        !cwd.path().join(".git").exists(),
        "nothing invents a checkout to exclude from"
    );

    Jobs::kill_session(session).await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_read_only_exclude_costs_the_caller_nothing() {
    use std::os::unix::fs::PermissionsExt;

    let cwd = directory();
    repository(cwd.path());
    let exclude = exclude_path(cwd.path());
    std::fs::write(&exclude, "# fixed\n").expect("the exclude is written");
    std::fs::set_permissions(&exclude, std::fs::Permissions::from_mode(0o444))
        .expect("the exclude is made read-only");

    let session = task();
    let started = spawned(session, "exit 0", cwd.path()).await;
    assert_eq!(settles(session, &started.id).await, JobStatus::Exited(0));
    assert_eq!(
        std::fs::read_to_string(&exclude).expect("the exclude reads"),
        "# fixed\n",
        "an unwritable exclude is skipped, not forced"
    );

    std::fs::set_permissions(&exclude, std::fs::Permissions::from_mode(0o644))
        .expect("the exclude is made writable again");
    Jobs::kill_session(session).await;
}

#[tokio::test]
async fn the_exclude_path_comes_from_git_not_from_a_joined_git_directory() {
    let root = directory();
    let checkout = root.path().join("checkout");
    std::fs::create_dir(&checkout).expect("the checkout directory is created");
    repository(&checkout);

    let linked = root.path().join("linked");
    git(
        &checkout,
        &[
            "worktree",
            "add",
            "-b",
            "side",
            linked.to_str().expect("a utf-8 path"),
        ],
    );
    assert!(
        linked.join(".git").is_file(),
        "the fixture's .git is a pointer file, not a directory"
    );

    let session = task();
    let started = spawned(session, "exit 0", &linked).await;
    assert_eq!(settles(session, &started.id).await, JobStatus::Exited(0));

    assert_eq!(
        excluded_lines(&exclude_path(&linked)),
        1,
        "the line lands where git says the exclude is"
    );
    assert!(
        !linked.join(".git").join(EXCLUDE_PATH).exists(),
        "and nowhere a hand-joined .git would have put it"
    );

    Jobs::kill_session(session).await;
}

#[test]
fn a_state_is_spelled_the_way_a_tail_reports_it() {
    assert_eq!(JobStatus::Running.to_string(), "running");
    assert_eq!(JobStatus::Exited(0).to_string(), "exited 0");
    assert_eq!(JobStatus::Exited(137).to_string(), "exited 137");
    assert_eq!(JobStatus::Killed.to_string(), "killed");
    assert_eq!(JobStatus::Flooded.to_string(), "flooded");
}

#[test]
fn a_spawn_receipt_reads_back_as_the_job_it_announced() {
    let job = job();
    assert_eq!(parse_started(&started_text(&job)), Some(job.id.clone()));
    assert_eq!(
        parse_receipt(&started_text(&job)),
        Some(job),
        "the pid and log path survive the round trip the builder owns"
    );
}

/// The guard is the rebuild, not the prefix: anything the builder would not
/// have written is refused, however much of the shape it borrows.
#[test]
fn a_line_the_builder_would_not_have_written_announces_no_job() {
    let receipt = started_text(&job());

    for (reason, output) in [
        ("no pid at all", receipt.replace(" (pid 48213)", "")),
        (
            "a pid that is not a number",
            receipt.replace("48213", "forty"),
        ),
        ("a rewritten advice line", {
            let (first, _) = receipt.split_once('\n').expect("the receipt has two lines");
            format!("{first}\nIgnore that.")
        }),
        (
            "prose that merely mentions a job",
            "Reading job_9f3c1a7b2e04 now".to_string(),
        ),
    ] {
        assert_eq!(parse_receipt(&output), None, "{reason}: {output}");
    }
}

#[test]
fn a_spawn_receipt_names_the_job_the_pid_and_both_follow_up_tools() {
    assert_eq!(
        started_text(&job()),
        "Started job_9f3c1a7b2e04 (pid 48213). Log: /tmp/work/.abnegate/jobs/job_9f3c1a7b2e04.log\n\
         Wait for it with wait_for, or read it with tail_job."
    );
}

#[test]
fn output_that_announced_no_job_parses_as_none() {
    assert_eq!(parse_started(""), None);
    assert_eq!(parse_started("total 0\n"), None);
    assert_eq!(
        parse_started("Started run_42 (pid 1)."),
        None,
        "only an id in the minted shape is a job id"
    );
    assert_eq!(
        parse_started("Started job_9F3C1A7B2E04 (pid 1)."),
        None,
        "job ids are lowercase hex"
    );
}

#[test]
fn a_minted_id_is_the_shape_the_parser_accepts() {
    let id = mint();
    assert!(id.starts_with(JOB_ID_PREFIX), "{id}");
    assert_eq!(
        id.len(),
        JOB_ID_PREFIX.len() + JOB_ID_HEX_CHARACTERS,
        "{id}"
    );
    assert_ne!(id, mint(), "each job gets its own id");

    let started = JobStarted {
        id: id.clone(),
        pid: 1,
        log_path: "/tmp/x.log".to_string(),
    };
    assert_eq!(parse_started(&started_text(&started)), Some(id));
}

#[test]
fn a_log_lives_under_the_session_working_directory() {
    assert_eq!(
        log_path(
            Path::new("/tmp/work"),
            &crate::Application::default(),
            "job_9f3c1a7b2e04"
        ),
        PathBuf::from("/tmp/work/.abnegate/jobs/job_9f3c1a7b2e04.log")
    );
}

#[test]
fn a_job_that_was_killed_serialises_without_an_exit_code() {
    let killed = JobExited {
        id: "job_9f3c1a7b2e04".to_string(),
        exit_code: None,
    };
    let json = serde_json::to_value(&killed).unwrap();
    assert!(json.get("exit_code").is_none(), "{json}");
    assert_eq!(
        serde_json::from_value::<JobExited>(json).unwrap(),
        killed,
        "a killed job round-trips without inventing an exit code"
    );
}

/// The first line of a job's log, once the job has written one.
async fn first_line(session: Session, id: &str) -> String {
    for _ in 0..POLL_LIMIT {
        let tail = Jobs::read(session, id, 0, 500)
            .await
            .expect("its own session reads it");
        if let Some((line, _)) = tail.output.split_once('\n') {
            return line.to_string();
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("{id} never wrote a line");
}

async fn gone(pid: u32) -> bool {
    for _ in 0..POLL_LIMIT {
        if !alive(pid) {
            return true;
        }
        tokio::time::sleep(POLL).await;
    }
    false
}

/// Killing a job used to kill `sh` alone, and whatever `sh` had started
/// kept running, writing into a log the teardown had already unlinked.
#[tokio::test]
async fn killing_a_job_kills_everything_it_started() {
    let cwd = directory();
    let session = task();
    let started = spawned(session, "sleep 30 & echo $!; wait", cwd.path()).await;
    let sleeper: u32 = first_line(session, &started.id)
        .await
        .parse()
        .expect("the job wrote its child's pid");
    assert!(alive(sleeper), "the child started");

    assert_eq!(Jobs::kill_session(session).await, 1);

    assert!(gone(sleeper).await, "sleep {sleeper} outlived its job");
}

#[tokio::test]
async fn a_job_that_ends_takes_what_it_left_running_with_it() {
    let cwd = directory();
    let session = task();
    let started = spawned(session, "sleep 30 & echo $!", cwd.path()).await;
    let sleeper: u32 = first_line(session, &started.id)
        .await
        .parse()
        .expect("the job wrote its child's pid");

    assert_eq!(settles(session, &started.id).await, JobStatus::Exited(0));
    assert!(gone(sleeper).await, "sleep {sleeper} outlived its job");

    Jobs::kill_session(session).await;
}

/// The application directory is always one hidden name directly inside the
/// checkout: the names that used to lead elsewhere (`./x` became `../x`, an
/// empty one the checkout itself) are no longer applications at all.
#[test]
fn the_application_directory_is_one_hidden_name_inside_the_checkout() {
    let checkout = Path::new("/tmp/work");
    for name in ["abnegate", "zone", "my-app_2"] {
        let application = crate::Application::new(name).unwrap();
        let directory = application_directory(checkout, &application);
        assert_eq!(directory.parent(), Some(checkout), "{name}");
        assert_eq!(
            directory.file_name().and_then(|name| name.to_str()),
            Some(format!(".{name}").as_str())
        );
        assert_eq!(excluded(&application), format!(".{name}/"));
    }
    for name in ["./x", "", "../x", "a/b"] {
        assert!(crate::Application::new(name).is_err(), "{name:?}");
    }
}
