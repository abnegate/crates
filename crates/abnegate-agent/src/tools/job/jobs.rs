use abnegate_exec::Proxy;
use dashmap::DashMap;
use std::future::Future;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, watch};

use super::entry::Job;
use super::limits::Limits;
use super::{
    EXCLUDE_PATH, JobCommand, JobExited, JobStarted, JobStatus, JobTail, KILL_TIMEOUT,
    MAX_CHARACTER_BYTES, UNAVAILABLE, excluded, log_directory, log_path, mint, missing,
};
use crate::tools::{Session, ToolContext};

pub(super) static JOBS: LazyLock<DashMap<String, Job>> = LazyLock::new(DashMap::new);

/// Every background child process this process owns.
pub struct Jobs;

impl Jobs {
    /// Start a job for the context's session, and hand back the receipt the
    /// model is shown.
    ///
    /// The context's `cwd` is the session's own working tree, and it is the
    /// only thing the log directory and the exclude write are ever derived
    /// from. Where the child runs is [`JobCommand::within`]'s to say, because
    /// that is the part a model chooses. The child is given the context's
    /// environment and nothing else.
    pub async fn spawn(command: &JobCommand, context: &ToolContext) -> Result<JobStarted, String> {
        Self::start(command, context, Limits::default()).await
    }

    /// Read a job's log from `since`, and say where the job has got to.
    ///
    /// The state is taken before the log rather than after, so a settled state
    /// never describes a slice read before the child's last write.
    pub async fn read(
        session: Session,
        id: &str,
        since: u64,
        max_characters: usize,
    ) -> Result<JobTail, String> {
        if session == Session::Detached {
            return Err(UNAVAILABLE.to_string());
        }
        let (log, state) = {
            let job = JOBS
                .get(id)
                .filter(|job| job.session == session)
                .ok_or_else(|| missing(id))?;
            (job.log.clone(), *job.state.borrow())
        };
        let unread = JobTail {
            output: String::new(),
            state,
            next: since,
        };

        let Ok(mut file) = tokio::fs::File::open(&log).await else {
            return Ok(unread);
        };
        if file.seek(SeekFrom::Start(since)).await.is_err() {
            return Ok(unread);
        }
        let mut buffer = Vec::with_capacity(max_characters);
        if file
            .take(max_characters as u64)
            .read_to_end(&mut buffer)
            .await
            .is_err()
        {
            return Ok(unread);
        }

        let (output, consumed) = decode(&buffer, state.settled());
        Ok(JobTail {
            output,
            state,
            next: since + consumed as u64,
        })
    }

    /// Claim a job's ending, to be awaited later.
    ///
    /// The claim is taken synchronously and the outcome is held in the job's
    /// own channel, so a job that ended before the caller got here resolves the
    /// future at once instead of stranding it.
    pub fn settled(
        session: Session,
        id: &str,
    ) -> Result<impl Future<Output = JobExited> + use<>, String> {
        if session == Session::Detached {
            return Err(UNAVAILABLE.to_string());
        }
        let state = {
            let job = JOBS
                .get(id)
                .filter(|job| job.session == session)
                .ok_or_else(|| missing(id))?;
            job.state.clone()
        };
        let id = id.to_string();
        Ok(async move { ended(state).await.exited(id) })
    }

    /// End every job this session started, take its log with it, and wait for
    /// each child to go.
    ///
    /// A job belongs to the turn or the run that started it, so this is the
    /// last thing either one does. Returns how many jobs it ended.
    pub async fn kill_session(session: Session) -> usize {
        let ids: Vec<String> = JOBS
            .iter()
            .filter(|job| job.session == session)
            .map(|job| job.key().clone())
            .collect();
        let claimed: Vec<(String, Job)> = ids.iter().filter_map(|id| JOBS.remove(id)).collect();

        let count = claimed.len();
        for (id, job) in claimed {
            let log = job.log;
            let _ = job.kill.send(());
            if tokio::time::timeout(KILL_TIMEOUT, ended(job.state))
                .await
                .is_err()
            {
                tracing::warn!(
                    job = %id,
                    seconds = KILL_TIMEOUT.as_secs(),
                    "A killed job has not been reaped yet; leaving it to the process"
                );
            }
            discard(&log).await;
        }
        count
    }

    /// The entry is published before the supervisor starts, so a child that
    /// exits immediately still has somewhere to record that it did: claim
    /// before publish.
    pub(super) async fn start(
        command: &JobCommand,
        context: &ToolContext,
        limits: Limits,
    ) -> Result<JobStarted, String> {
        let session = context.session;
        if session == Session::Detached {
            return Err(UNAVAILABLE.to_string());
        }
        let checkout = context.working_directory.as_path();
        let directory = log_directory(checkout, &context.application);

        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|error| format!("Cannot create {}: {error}", directory.display()))?;
        if matches!(session, Session::Task(_)) {
            exclude(checkout, &context.application).await;
        }

        let id = mint();
        let log = log_path(checkout, &context.application, &id);
        let file = std::fs::File::create(&log)
            .map_err(|error| format!("Cannot create the job log: {error}"))?;
        let errors = file
            .try_clone()
            .map_err(|error| format!("Cannot create the job log: {error}"))?;

        let mut process = Command::new(&command.program);
        process
            .args(&command.arguments)
            .current_dir(command.directory.as_deref().unwrap_or(checkout))
            .stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::from(errors))
            .kill_on_drop(true);
        process.env_clear();
        for (key, value) in &context.env {
            process.env(key, value);
        }
        Proxy::from_env().apply(&mut process);

        let child = process
            .spawn()
            .map_err(|error| format!("Failed to start the job: {error}"))?;
        let pid = child.id().unwrap_or_default();

        let (sender, state) = watch::channel(JobStatus::Running);
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
        tokio::spawn(supervise(child, log.clone(), limits, killed, sender));

        Ok(JobStarted {
            id,
            pid,
            log_path: log.to_string_lossy().into_owned(),
        })
    }
}

/// Hold a job to its limits, and record how it ended.
///
/// Polled in order, not at random: a child that exits in the same tick as its
/// deadline elapses has exited, and reporting that as a kill would hand the
/// model a failure it did not have.
async fn supervise(
    mut child: Child,
    log: PathBuf,
    limits: Limits,
    kill: oneshot::Receiver<()>,
    state: watch::Sender<JobStatus>,
) {
    let outcome = {
        let flood = flooded(&log, limits.log_bytes, limits.log_check);
        tokio::select! {
            biased;
            status = child.wait() => match status {
                Ok(status) => status.code().map_or(JobStatus::Killed, JobStatus::Exited),
                Err(_) => JobStatus::Killed,
            },
            _ = kill => JobStatus::Killed,
            _ = tokio::time::sleep(limits.lifetime) => JobStatus::Killed,
            _ = flood => JobStatus::Flooded,
        }
    };
    if !matches!(outcome, JobStatus::Exited(_)) {
        let _ = child.kill().await;
    }
    state.send_replace(outcome);
}

/// Resolves once the log has passed the ceiling it is allowed.
async fn flooded(log: &Path, ceiling: u64, interval: Duration) {
    loop {
        tokio::time::sleep(interval).await;
        if tokio::fs::metadata(log)
            .await
            .is_ok_and(|log| log.len() > ceiling)
        {
            return;
        }
    }
}

/// Take a job's log with it, and the directories it needed once they are
/// empty.
///
/// Nothing can read the log after this: the registry entry it was reached
/// through is already gone, and a chat's logs sit in the operator's own
/// checkout, where the exclude write is skipped by design. A job the reaper
/// killed for its lifetime or for flooding keeps its log until here, so a
/// `tail_job` in the meantime still reports how it ended.
///
/// `remove_dir` on a directory something else is using fails, which is the
/// whole of the emptiness check.
async fn discard(log: &Path) {
    let _ = tokio::fs::remove_file(log).await;
    let Some(jobs) = log.parent() else {
        return;
    };
    if tokio::fs::remove_dir(jobs).await.is_err() {
        return;
    }
    if let Some(application) = jobs.parent() {
        let _ = tokio::fs::remove_dir(application).await;
    }
}

/// Resolves once the job has stopped, immediately if it already had.
async fn ended(mut state: watch::Receiver<JobStatus>) -> JobStatus {
    loop {
        let current = *state.borrow_and_update();
        if current.settled() {
            return current;
        }
        if state.changed().await.is_err() {
            return JobStatus::Killed;
        }
    }
}

/// Keep job logs out of a task run's diff.
///
/// Task runs only, and only in the run's own checkout: a chat works in the
/// host checkout, whose `.git` may be a pointer into a directory every
/// worktree of the repository shares, and one chat's job must not write into
/// all of them. It is also why the directory asked about is the session's own
/// tree and never one the command named — a run naming somebody else's
/// checkout would otherwise write into a repository it does not own. Every
/// failure — not a checkout, no git, an unwritable file — is a silent skip,
/// because a background job is worth more to the caller than a tidy diff.
async fn exclude(checkout: &Path, application: &str) {
    let Ok(resolved) = Command::new("git")
        .arg("rev-parse")
        .arg("--git-path")
        .arg(EXCLUDE_PATH)
        .current_dir(checkout)
        .stdin(Stdio::null())
        .output()
        .await
    else {
        return;
    };
    if !resolved.status.success() {
        return;
    }
    let Ok(resolved) = std::str::from_utf8(&resolved.stdout) else {
        return;
    };
    let path = checkout.join(resolved.trim());
    // A checkout cloned with an empty template has no `info` directory, and an
    // append cannot create the parent it is missing.
    if let Some(parent) = path.parent()
        && tokio::fs::create_dir_all(parent).await.is_err()
    {
        return;
    }

    let line = excluded(application);
    let existing = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    if existing.lines().any(|existing| existing.trim() == line) {
        return;
    }
    let opening = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let Ok(mut file) = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
    else {
        return;
    };
    let _ = file
        .write_all(format!("{opening}{line}\n").as_bytes())
        .await;
}

/// Take whole characters only, and say how many bytes that spent.
///
/// A character split by the read budget is left for the next read rather than
/// replaced here, so a cursor walked across several reads loses nothing. Once
/// the job has stopped no more bytes are coming, so a trailing fragment is
/// spent rather than waited on forever.
fn decode(buffer: &[u8], settled: bool) -> (String, usize) {
    match std::str::from_utf8(buffer) {
        Ok(text) => (text.to_string(), buffer.len()),
        Err(error) => {
            let whole = error.valid_up_to();
            if whole == 0 && (settled || buffer.len() > MAX_CHARACTER_BYTES) {
                return (String::from_utf8_lossy(buffer).into_owned(), buffer.len());
            }
            (
                String::from_utf8_lossy(&buffer[..whole]).into_owned(),
                whole,
            )
        }
    }
}
