use std::panic;
use std::path::Path;
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
use crate::login::Cli;
use crate::login::LoginError;
use crate::login::codex::DeviceLogin;
use crate::login::codex::HOME;
use crate::login::codex::LOGIN;
use crate::login::codex::Prompt;
use crate::login::codex::Staging;
use crate::login::codex::prompt::NO_LINK;
use crate::login::codex::signed_in;
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

const SPENT: &str = "a device sign-in's outcome was waited on after it resolved";

/// How far ahead a deadline is set when its limit is beyond what this
/// host's clock can count to: about 30 years, as Tokio's own timers do.
const FAR: Duration = Duration::from_secs(86_400 * 365 * 30);

/// A pending `codex login --device-auth`: the prompt it printed, and the
/// task that sees it through.
///
/// It is cancelled only on purpose: by [`cancel`](Self::cancel), or by
/// dropping it. Either stops codex, terminating its process group and
/// killing what is left of it after a grace period, and removes the staging
/// directory; a drop does so in the background, `cancel` before it returns.
/// Waiting on [`outcome`](Self::outcome) cancels nothing, however the wait
/// ends. Nothing of it can be moved out and the rest dropped by accident:
///
/// ```compile_fail,E0451
/// use abnegate_agent_cli::login::codex::Device;
///
/// fn shown(device: Device) {
///     let Device { prompt, .. } = device;
///     let _ = prompt;
/// }
/// # let _ = shown;
/// ```
#[derive(Debug)]
pub struct Device {
    prompt: Prompt,
    outcome: Option<JoinHandle<Result<Staging, LoginError>>>,
    cancel: Option<oneshot::Sender<()>>,
}

impl Device {
    /// The link and code for a person to enter.
    pub fn prompt(&self) -> &Prompt {
        &self.prompt
    }

    /// Resolves once codex has exited or been stopped. `Ok` holds the login
    /// codex saved, for the caller to promote or keep; any other ending has
    /// already removed the staging directory.
    ///
    /// Cancel safe: a wait given up, such as by a timeout or another branch
    /// of a `select!`, leaves the sign-in running, to be waited on again.
    ///
    /// # Errors
    ///
    /// [`LoginError::Failed`] in codex's own words when it refuses the
    /// sign-in, or exits cleanly having saved no login,
    /// [`LoginError::Expired`] when nobody entered the code before it
    /// expired, [`LoginError::Unreadable`] when codex cannot be waited for,
    /// and [`LoginError::Cancelled`] when the runtime stopped the sign-in.
    /// No reason repeats the one-time code or a secret.
    ///
    /// # Panics
    ///
    /// When it is called again after it resolved, as a future polled after
    /// it completed does, and when the task seeing the sign-in through
    /// panicked.
    pub async fn outcome(&mut self) -> Result<Staging, LoginError> {
        let outcome = self.outcome.as_mut().expect(SPENT);
        let joined = outcome.await;
        self.outcome = None;
        match joined {
            Ok(result) => result,
            Err(error) if error.is_panic() => panic::resume_unwind(error.into_panic()),
            Err(_) => Err(LoginError::Cancelled),
        }
    }

    /// Stops codex and discards the attempt, returning once codex and
    /// everything it forked have been stopped and the staging directory is
    /// gone. A login codex saved before it was stopped is discarded with it.
    pub async fn cancel(mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(outcome) = self.outcome.take() {
            let _ = outcome.await;
        }
    }

    pub(crate) async fn start(login: &DeviceLogin, root: &Path) -> Result<Self, LoginError> {
        let cli = login.cli();
        let executable = cli.executable();
        let limits = login.limits();
        let staging = Staging::create(root)?;
        let mut command = Process::command(cli, LOGIN);
        command.env(HOME, staging.path());
        let mut process = Process::spawn(command, executable)?;
        let stderr = tokio::spawn(output::read(process.stderr()));
        let Some(mut stdout) = process.stdout() else {
            process.stop().await;
            stderr.abort();
            return Err(unreadable("codex's output could not be read"));
        };
        let deadline = after(limits.prompt);

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
        let expiry = after(lifetime.saturating_add(limits.grace));
        let (cancel, cancelled) = oneshot::channel();
        let outcome = tokio::spawn(watch(
            process,
            stdout,
            stderr,
            staging,
            cancelled,
            expiry,
            prompt.clone(),
            cli.clone(),
        ));
        Ok(Self {
            prompt,
            outcome: Some(outcome),
            cancel: Some(cancel),
        })
    }
}

/// `limit` from now, or [`FAR`] from now when the clock cannot count that
/// far.
fn after(limit: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(limit).unwrap_or_else(|| now + FAR)
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

/// Sees a pending sign-in through: the caller cancels, codex exits, or the
/// code expires, in that order when more than one has happened. A codex the
/// expiry stops having already exited cleanly with a login saved is a
/// sign-in like any other.
async fn watch(
    mut process: Process,
    stdout: ChildStdout,
    stderr: JoinHandle<String>,
    staging: Staging,
    cancelled: oneshot::Receiver<()>,
    expiry: Instant,
    prompt: Prompt,
    cli: Cli,
) -> Result<Staging, LoginError> {
    let drain = tokio::spawn(drain(stdout));
    let outcome = tokio::select! {
        biased;
        _ = cancelled => {
            process.stop().await;
            stderr.abort();
            Err(LoginError::Cancelled)
        }
        status = process.wait() => match status {
            Ok(status) if status.success() => {
                stderr.abort();
                staging.saved()
            }
            Ok(status) => Err(concealed(failure(cli.executable(), &collect(stderr).await, status), &prompt)),
            Err(error) => {
                stderr.abort();
                Err(unreadable(&format!("codex could not be waited for: {error}")))
            }
        },
        () = sleep_until(expiry) => {
            let status = process.stop().await;
            stderr.abort();
            if status.is_some_and(|status| status.success()) && signed_in(staging.path()) {
                Ok(staging)
            } else {
                Err(LoginError::Expired { expires_at: prompt.expires_at })
            }
        }
    };
    drain.abort();
    outcome.map_err(|error| cli.scrubbed(error))
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
