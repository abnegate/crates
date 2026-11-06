use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use abnegate_secret::REDACTED;
use chrono::Utc;
use tokio::io::AsyncReadExt;
use tokio::process::ChildStdout;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio::time::sleep_until;
use tokio::time::timeout_at;

use crate::lines::Lines;
use crate::login::LoginError;
use crate::login::codex::DeviceLogin;
use crate::login::codex::HOME;
use crate::login::codex::LOGIN;
use crate::login::codex::Prompt;
use crate::login::codex::Staging;
use crate::login::codex::prompt::NO_LINK;
use crate::login::output;
use crate::login::output::READ;
use crate::login::output::collect;
use crate::login::output::failure;
use crate::login::output::strip;
use crate::login::process::Process;

/// The longest line codex may print while its prompt is read.
pub(crate) const LINE: usize = 4 * 1024;

/// How much codex may print before its prompt is whole.
const PRINTED: usize = 64 * 1024;

/// A pending `codex login --device-auth`: the prompt it printed, and the
/// task that sees it through.
#[derive(Debug)]
#[non_exhaustive]
pub struct Device {
    /// The link and code for a person to enter.
    pub prompt: Prompt,
    /// Resolves once codex has exited or been stopped. `Ok` holds the login
    /// codex saved, for the caller to promote or keep; any other ending has
    /// already removed the staging directory.
    pub outcome: JoinHandle<Result<Staging, LoginError>>,
    /// Sending on it, or dropping it, stops codex, terminating its process
    /// group and killing what is left of it after a grace period, and
    /// discards the attempt.
    pub cancel: oneshot::Sender<()>,
}

impl Device {
    /// A pending sign-in showing `prompt`, seen through by `outcome` and
    /// stopped through `cancel`.
    pub fn new(
        prompt: Prompt,
        outcome: JoinHandle<Result<Staging, LoginError>>,
        cancel: oneshot::Sender<()>,
    ) -> Self {
        Self {
            prompt,
            outcome,
            cancel,
        }
    }

    pub(crate) async fn start(login: &DeviceLogin, root: &Path) -> Result<Self, LoginError> {
        let executable = login.executable();
        let limits = login.limits();
        let staging = Staging::create(root)?;
        let mut command = Process::command(executable, LOGIN, login.environment());
        command.env(HOME, staging.path());
        let mut process = Process::spawn(command, executable)?;
        let stderr = tokio::spawn(output::read(process.stderr()));
        let Some(mut stdout) = process.stdout() else {
            process.stop().await;
            stderr.abort();
            return Err(unreadable("codex's output could not be read"));
        };
        let deadline = Instant::now() + limits.prompt;

        let prompt = match read(&mut stdout, deadline, limits.prompt).await {
            Ok(Some(prompt)) => prompt,
            Ok(None) => return Err(ended(&mut process, stderr, deadline, executable).await),
            Err(error) => {
                process.stop().await;
                stderr.abort();
                return Err(error);
            }
        };

        let lifetime = (prompt.expires_at - Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO);
        let Some(expiry) = Instant::now().checked_add(lifetime.saturating_add(limits.grace)) else {
            process.stop().await;
            stderr.abort();
            return Err(unreadable(
                "codex printed an expiry too far away to wait for",
            ));
        };
        let (cancel, cancelled) = oneshot::channel();
        let outcome = tokio::spawn(watch(
            process,
            stdout,
            stderr,
            staging,
            cancelled,
            expiry,
            prompt.clone(),
            executable.to_path_buf(),
        ));
        Ok(Self::new(prompt, outcome, cancel))
    }
}

/// Codex's stdout, read until it holds a whole prompt. `Ok(None)` means
/// codex closed it first, which it does only on its way out.
async fn read(
    stdout: &mut ChildStdout,
    deadline: Instant,
    limit: Duration,
) -> Result<Option<Prompt>, LoginError> {
    let mut lines = Lines::new(LINE);
    let mut text = String::new();
    let mut buffer = [0_u8; READ];
    let mut missing = NO_LINK;

    loop {
        let read = match timeout_at(deadline, stdout.read(&mut buffer)).await {
            Ok(Ok(read)) => read,
            Ok(Err(error)) => {
                return Err(unreadable(&format!(
                    "codex's output could not be read: {error}"
                )));
            }
            Err(_) => {
                return Err(unreadable(&format!(
                    "codex printed {missing} within {limit:?}"
                )));
            }
        };
        if read == 0 {
            if let Ok(Some(line)) = lines.flush() {
                append(&mut text, &line);
            }
            return Ok(Prompt::read(&text, Utc::now()).ok());
        }
        lines.extend(&buffer[..read]);
        loop {
            match lines.take() {
                Ok(Some(line)) => append(&mut text, &line),
                Ok(None) => break,
                Err(_) => {
                    return Err(unreadable(&format!(
                        "codex printed a line longer than {LINE} bytes instead of a sign-in prompt"
                    )));
                }
            }
        }
        if text.len() > PRINTED {
            return Err(unreadable(&format!(
                "codex printed more than {PRINTED} bytes without a sign-in prompt"
            )));
        }
        match Prompt::read(&text, Utc::now()) {
            Ok(prompt) => return Ok(Some(prompt)),
            Err(part) => missing = part,
        }
    }
}

fn append(text: &mut String, line: &str) {
    text.push_str(&strip(line));
    text.push('\n');
}

fn unreadable(reason: &str) -> LoginError {
    LoginError::Unreadable {
        reason: reason.to_string(),
    }
}

/// Why codex exited before it printed a prompt, in its own words when it
/// gave any.
async fn ended(
    process: &mut Process,
    stderr: JoinHandle<String>,
    deadline: Instant,
    executable: &Path,
) -> LoginError {
    match timeout_at(deadline, process.wait()).await {
        Ok(Ok(status)) => failure(executable, &collect(stderr).await, status),
        Ok(Err(error)) => {
            process.stop().await;
            stderr.abort();
            unreadable(&format!("codex could not be waited for: {error}"))
        }
        Err(_) => {
            process.stop().await;
            stderr.abort();
            unreadable("codex closed its output without printing a sign-in prompt")
        }
    }
}

/// Sees a pending sign-in through: codex exits, the caller cancels, or the
/// code expires.
async fn watch(
    mut process: Process,
    stdout: ChildStdout,
    stderr: JoinHandle<String>,
    staging: Staging,
    cancelled: oneshot::Receiver<()>,
    expiry: Instant,
    prompt: Prompt,
    executable: PathBuf,
) -> Result<Staging, LoginError> {
    let drain = tokio::spawn(drain(stdout));
    let outcome = tokio::select! {
        biased;
        _ = cancelled => {
            process.stop().await;
            stderr.abort();
            Err(LoginError::Cancelled)
        }
        () = sleep_until(expiry) => {
            process.stop().await;
            stderr.abort();
            Err(LoginError::Expired { expires_at: prompt.expires_at })
        }
        status = process.wait() => match status {
            Ok(status) if status.success() => {
                stderr.abort();
                staging.saved()
            }
            Ok(status) => Err(concealed(failure(&executable, &collect(stderr).await, status), &prompt)),
            Err(error) => {
                stderr.abort();
                Err(unreadable(&format!("codex could not be waited for: {error}")))
            }
        },
    };
    drain.abort();
    outcome
}

/// `error` without the prompt's one-time code, should codex have repeated it.
fn concealed(error: LoginError, prompt: &Prompt) -> LoginError {
    match error {
        LoginError::Failed { reason } if !prompt.user_code.is_empty() => LoginError::Failed {
            reason: reason.replace(&prompt.user_code, REDACTED),
        },
        error => error,
    }
}

/// Codex prints nothing more once its prompt is out, but a child blocked on
/// a full pipe never exits, so whatever it does print is read and dropped.
async fn drain(mut stdout: ChildStdout) {
    let mut buffer = [0_u8; READ];
    while let Ok(read) = stdout.read(&mut buffer).await {
        if read == 0 {
            break;
        }
    }
}
