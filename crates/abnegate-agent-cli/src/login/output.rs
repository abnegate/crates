use std::borrow::Cow;
use std::path::Path;
use std::process::ExitStatus;
use std::sync::LazyLock;
use std::time::Duration;

use abnegate_exec::executor::GRACE_PERIOD;
use abnegate_secret::redact;
use regex::Regex;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::login::LoginError;
use crate::login::process::Process;

/// How much of each stream is kept.
pub(crate) const KEPT: usize = 64 * 1024;

/// How much is read at a time.
pub(crate) const READ: usize = 8 * 1024;

/// How long a short-lived command such as `codex logout` may run.
const TIMEOUT: Duration = Duration::from_secs(30);

static ESCAPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").expect("the escape pattern is a valid regex")
});

/// What a short-lived sign-in command printed, with its colour codes
/// stripped, and how it exited.
#[derive(Debug)]
pub(crate) struct Output {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

impl Output {
    pub(crate) async fn capture(command: Command, executable: &Path) -> Result<Self, LoginError> {
        Self::capture_within(command, executable, TIMEOUT).await
    }

    pub(crate) async fn capture_within(
        command: Command,
        executable: &Path,
        limit: Duration,
    ) -> Result<Self, LoginError> {
        let mut process = Process::spawn(command, executable)?;
        let stdout = tokio::spawn(read(process.stdout()));
        let stderr = tokio::spawn(read(process.stderr()));
        let status = match timeout(limit, process.wait()).await {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => {
                process.stop().await;
                stdout.abort();
                stderr.abort();
                return Err(LoginError::Unreadable {
                    reason: format!("{} could not be waited for: {error}", executable.display()),
                });
            }
            Err(_) => {
                process.stop().await;
                stdout.abort();
                stderr.abort();
                return Err(LoginError::Unreadable {
                    reason: format!("{} did not finish within {limit:?}", executable.display()),
                });
            }
        };
        Ok(Self {
            status,
            stdout: strip(&collect(stdout).await).into_owned(),
            stderr: strip(&collect(stderr).await).into_owned(),
        })
    }

    pub(crate) fn failed(&self, executable: &Path) -> LoginError {
        failure(executable, &self.stderr, self.status)
    }
}

/// The CLI's own words for why it failed, with anything shaped like a
/// credential redacted, or how it exited when it gave none.
pub(crate) fn failure(executable: &Path, stderr: &str, status: ExitStatus) -> LoginError {
    let stripped = strip(stderr);
    let words = stripped.trim();
    let reason = if words.is_empty() {
        format!(
            "{} exited with {status} and gave no reason",
            executable.display()
        )
    } else {
        redact(words).into_owned()
    };
    LoginError::Failed { reason }
}

/// Everything `stream` carries until it closes, keeping the first [`KEPT`]
/// bytes. It goes on reading past them, because a child blocked on a full
/// pipe never exits.
pub(crate) async fn read(stream: Option<impl AsyncRead + Unpin>) -> String {
    let Some(mut stream) = stream else {
        return String::new();
    };
    let mut kept = Vec::new();
    let mut buffer = [0_u8; READ];
    while let Ok(read) = stream.read(&mut buffer).await {
        if read == 0 {
            break;
        }
        let room = KEPT.saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..read.min(room)]);
    }
    String::from_utf8_lossy(&kept).into_owned()
}

/// What `reader` read, given a bounded wait: a process that escaped the
/// CLI's group can hold its pipe open long after the CLI has exited.
pub(crate) async fn collect(mut reader: JoinHandle<String>) -> String {
    match timeout(GRACE_PERIOD, &mut reader).await {
        Ok(text) => text.unwrap_or_default(),
        Err(_) => {
            reader.abort();
            String::new()
        }
    }
}

/// `text` without the colour codes a CLI prints even when its output is not
/// a terminal.
pub(crate) fn strip(text: &str) -> Cow<'_, str> {
    ESCAPE.replace_all(text, "")
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::test_support::cli::ended;
    use crate::test_support::cli::environment;
    use crate::test_support::cli::fake;
    use crate::test_support::cli::process;

    #[tokio::test]
    async fn a_command_that_never_finishes_is_stopped_at_its_limit() {
        let directory = TempDir::new().expect("a temporary directory");
        let record = directory.path().join("process");
        let cli = fake(
            &directory,
            "status",
            &format!("echo $$ > '{}'\nexec sleep 60", record.display()),
        );

        let error = Output::capture_within(
            Process::command(&cli, &["status"], &environment()),
            &cli,
            Duration::from_secs(1),
        )
        .await
        .expect_err("a command that never finishes");

        assert!(
            matches!(&error, LoginError::Unreadable { reason, .. } if reason.contains("did not finish")),
            "{error:?}"
        );
        assert!(ended(process(&record)), "the command was left running");
    }

    #[tokio::test]
    async fn output_past_what_is_kept_is_still_read_so_the_command_can_exit() {
        let directory = TempDir::new().expect("a temporary directory");
        let cli = fake(
            &directory,
            "status",
            "head -c 1048576 /dev/zero | tr '\\0' 'x'\necho 'done' >&2\nexit 3",
        );

        let output = Output::capture_within(
            Process::command(&cli, &["status"], &environment()),
            &cli,
            Duration::from_secs(20),
        )
        .await
        .expect("the command's output");

        assert_eq!(output.stdout.len(), KEPT);
        assert_eq!(output.stderr, "done\n");
        assert_eq!(output.status.code(), Some(3));
    }

    #[tokio::test]
    async fn colour_codes_never_reach_what_was_captured() {
        let directory = TempDir::new().expect("a temporary directory");
        let cli = fake(
            &directory,
            "status",
            "printf '\\033[94mLogged in using ChatGPT\\033[0m\\n' >&2",
        );

        let output = Output::capture_within(
            Process::command(&cli, &["status"], &environment()),
            &cli,
            Duration::from_secs(20),
        )
        .await
        .expect("the command's output");

        assert_eq!(output.stderr, "Logged in using ChatGPT\n");
    }

    #[tokio::test]
    async fn a_missing_cli_is_unavailable() {
        let missing = Path::new("/nonexistent/codex");

        let error = Output::capture_within(
            Process::command(missing, &["status"], &environment()),
            missing,
            Duration::from_secs(20),
        )
        .await
        .expect_err("a missing CLI");

        assert!(
            matches!(&error, LoginError::Unavailable { executable, .. } if executable == missing),
            "{error:?}"
        );
    }

    #[test]
    fn a_credential_in_the_clis_words_is_redacted() {
        use std::os::unix::process::ExitStatusExt;

        let key = "sk-proj-Q7vR2mX9kL4pN8sT1wY6zB3cF5hJ0dGa";
        let error = failure(
            Path::new("/usr/local/bin/codex"),
            &format!("Error: the key {key} was refused\n"),
            ExitStatus::from_raw(1 << 8),
        );

        let LoginError::Failed { reason, .. } = &error else {
            panic!("{error:?}");
        };
        assert!(!reason.contains(key), "{reason}");
        assert!(reason.starts_with("Error: the key"), "{reason}");
    }
}
