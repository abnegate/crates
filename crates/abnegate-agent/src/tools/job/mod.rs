//! Background shell jobs: the registry that owns every child process this
//! process started, the shapes a spawn is reported in, and the limits one runs
//! under.
//!
//! A job outlives the tool call that started it but not the process that owns
//! it, so nothing here is persisted: a row that survived a restart the child
//! did not would claim a durability the thing has never had. One process owns
//! the registry and every child in it.
//!
//! The spawn receipt is also the only channel a tool has to the chat layer —
//! `ToolResult` carries no structured detail — so the text is built and read
//! back in this one file, and a round-trip test keeps the two ends honest.

mod command;
mod entry;
mod exited;
mod jobs;
mod limits;
mod started;
mod status;
mod tail;
#[cfg(test)]
mod tests;

pub use command::{JobCommand, SHELL, SHELL_COMMAND_FLAG};
pub use exited::JobExited;
pub use jobs::Jobs;
pub use started::JobStarted;
pub use status::JobStatus;
pub use tail::JobTail;

use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

use crate::Application;

pub const TAIL_JOB: &str = "tail_job";

/// The tool an embedding application registers to block until a background
/// job ends, built on [`Jobs::settled`].
///
/// Receipts and schemas point the model at it by this name, so an application
/// that registers such a tool registers it under this name.
pub const WAIT_FOR: &str = "wait_for";

/// Longest a background job may run before it is killed.
///
/// The same cap a foreground shell command is held to: backgrounding is a way
/// to stop blocking the loop, not a way to buy a longer command.
pub const MAX_JOB_LIFETIME: Duration =
    Duration::from_secs(super::command::MAX_SHELL_TIMEOUT_SECONDS);

/// Ceiling on a job's log file, past which the job is killed and reported as
/// flooded rather than truncated and reported as fine.
pub const MAX_JOB_LOG_BYTES: u64 = 64 * 1024 * 1024;

/// Where job logs live, inside the application directory of the session's own
/// working tree.
///
/// Inside the checkout rather than a shared temp directory: `run_command`'s
/// allow-list reaches `cat`, `tail` and `grep` with unconstrained paths, so a
/// shared location would be a new cross-workspace read surface.
pub const JOB_LOG_DIRECTORY: &str = "jobs";

/// Longest a session teardown waits for one killed child to be reaped.
///
/// The chat's teardown is a single point that holds the chat's generation
/// permit, and a child wedged in uninterruptible I/O is never reaped at all,
/// so the wait is bounded and the lag is logged. The kill has already been
/// sent by then: what is given up is the confirmation, not the signal.
const KILL_TIMEOUT: Duration = Duration::from_secs(5);

/// What a context with no chat and no run is told when it reaches for a job.
pub const UNAVAILABLE: &str = "Background jobs are not available in this context.";

const JOB_LOG_EXTENSION: &str = "log";

/// Distinguishes a job id from a run id at a glance, and keeps it short enough
/// to carry between calls.
const JOB_ID_PREFIX: &str = "job_";
const JOB_ID_HEX_CHARACTERS: usize = 12;

const STARTED_PREFIX: &str = "Started ";

/// How often a running job's log is measured against its ceiling.
///
/// Measuring costs one `stat` and the ceiling is there to protect the disk, so
/// the interval is short enough that even a pathological writer puts little
/// past it before the kill lands.
const LOG_CHECK_INTERVAL: Duration = Duration::from_millis(250);

/// Widest a single UTF-8 character is, and so the most a read holds back while
/// waiting for the rest of one.
const MAX_CHARACTER_BYTES: usize = 4;

/// Asked of git rather than joined onto `.git`: in a linked worktree `.git` is
/// a pointer file and the real exclude lives in the common directory.
const EXCLUDE_PATH: &str = "info/exclude";

/// The directory the tools keep their own files in, inside a working tree.
pub fn application_directory(checkout: &Path, application: &Application) -> PathBuf {
    checkout.join(application.directory())
}

/// Where job logs live under the session's own working tree.
pub fn log_directory(checkout: &Path, application: &Application) -> PathBuf {
    application_directory(checkout, application).join(JOB_LOG_DIRECTORY)
}

/// The line that keeps a task run's job logs out of its diff.
fn excluded(application: &Application) -> String {
    format!("{}/", application.directory())
}

fn missing(id: &str) -> String {
    format!("No job {id} in this session.")
}

/// Mint a job id: `job_` and twelve lowercase hex characters.
pub fn mint() -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("{JOB_ID_PREFIX}{}", &hex[..JOB_ID_HEX_CHARACTERS])
}

/// Where the log for `id` belongs, under the session's own working tree.
pub fn log_path(checkout: &Path, application: &Application, id: &str) -> PathBuf {
    log_directory(checkout, application).join(format!("{id}.{JOB_LOG_EXTENSION}"))
}

/// What a backgrounded shell call returns to the model.
pub fn started_text(job: &JobStarted) -> String {
    format!(
        "{STARTED_PREFIX}{} (pid {}). Log: {}\nWait for it with {WAIT_FOR}, or read it with {TAIL_JOB}.",
        job.id, job.pid, job.log_path
    )
}

/// Read a job id back out of a spawn receipt, for a caller holding only the
/// tool's own output.
pub fn parse_started(output: &str) -> Option<String> {
    output
        .lines()
        .next()?
        .strip_prefix(STARTED_PREFIX)?
        .split_whitespace()
        .next()
        .filter(|candidate| is_job_id(candidate))
        .map(str::to_string)
}

/// Where a spawn receipt keeps the pid, either side of it.
const RECEIPT_PID_OPENING: &str = " (pid ";
const RECEIPT_PID_CLOSING: &str = "). Log: ";

/// Read a whole job back out of a spawn receipt.
///
/// A caller holding only the tool's own output has nowhere else to look: the
/// registry keeps no pid and a `ToolResult` has no slot for one. What is read
/// back is therefore checked by rebuilding the receipt from it, so a change to
/// [`started_text`] stops this recognising the line rather than reporting a job
/// with the wrong pid. Reading lives beside writing for the same reason: the
/// format is this module's, and a reader that re-derived it elsewhere would
/// drift from the builder in silence.
pub fn parse_receipt(output: &str) -> Option<JobStarted> {
    let id = parse_started(output)?;
    let (announced, rest) = output.lines().next()?.split_once(RECEIPT_PID_OPENING)?;
    if !announced.ends_with(&id) {
        return None;
    }
    let (pid, log_path) = rest.split_once(RECEIPT_PID_CLOSING)?;
    let job = JobStarted {
        id,
        pid: pid.parse().ok()?,
        log_path: log_path.to_string(),
    };
    (started_text(&job) == output).then_some(job)
}

fn is_job_id(candidate: &str) -> bool {
    candidate.strip_prefix(JOB_ID_PREFIX).is_some_and(|hex| {
        hex.len() == JOB_ID_HEX_CHARACTERS
            && hex
                .chars()
                .all(|character| matches!(character, '0'..='9' | 'a'..='f'))
    })
}
