use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use abnegate_secret::SecretValue;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::modality::vendor::anthropic::failure::Failure;
use crate::provider::ProviderError;

const EXECUTABLE: &str = "claude";
const OAUTH_VARIABLE: &str = "CLAUDE_CODE_OAUTH_TOKEN";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// The most of the CLI's JSON envelope that is read. An answer is a few
/// kilobytes; a CLI writing more than this has gone wrong.
const MAXIMUM_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;
/// The most of the CLI's diagnostics kept for an error message.
const MAXIMUM_DIAGNOSTIC_BYTES: u64 = 64 * 1024;

/// The installed Claude Code CLI, run once per call.
///
/// The prompt is written to the child's stdin rather than its argument list,
/// so a prompt of any length fits and one that starts with `-` is never read
/// as a flag. The child is killed if the call is dropped or overruns its
/// deadline, and its output is read only up to a bound. A run that exits
/// unsuccessfully is reported as its JSON envelope describes it, and as its
/// diagnostics only when there is no envelope.
#[derive(Debug, Clone)]
pub(crate) struct Cli {
    executable: PathBuf,
    timeout: Duration,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            executable: PathBuf::from(EXECUTABLE),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl Cli {
    pub(crate) fn with_executable(mut self, executable: impl Into<PathBuf>) -> Self {
        self.executable = executable.into();
        self
    }

    pub(crate) fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Run the CLI with `arguments`, `prompt` on stdin and `token`, when
    /// there is one, in its environment, and return what it wrote to stdout.
    pub(crate) async fn run(
        &self,
        provider: &str,
        arguments: &[String],
        prompt: &str,
        token: Option<&SecretValue>,
    ) -> Result<Vec<u8>, ProviderError> {
        let executable = self.executable.display().to_string();
        let mut command = Command::new(&self.executable);
        command
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(token) = token {
            command.env(OAUTH_VARIABLE, token.expose());
        }

        let mut child = command.spawn().map_err(|error| {
            ProviderError::unavailable(
                provider,
                &executable,
                format!("{error}. Install it, or choose another provider."),
            )
        })?;
        let (Some(mut stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(ProviderError::unavailable(
                provider,
                &executable,
                "its standard streams could not be attached",
            ));
        };

        let exchange = async {
            let write = async {
                // A child that exits without reading its prompt closes the
                // pipe; its exit status says why, so the write error does not.
                let _ = stdin.write_all(prompt.as_bytes()).await;
                drop(stdin);
                Ok::<(), ProviderError>(())
            };
            let (_, output, diagnostics) = tokio::try_join!(
                write,
                bounded(provider, stdout, MAXIMUM_OUTPUT_BYTES),
                truncated(stderr, MAXIMUM_DIAGNOSTIC_BYTES),
            )?;
            let status = child
                .wait()
                .await
                .map_err(|error| ProviderError::unavailable(provider, &executable, error))?;
            if !status.success() {
                return Err(
                    Failure::exited(&output, &diagnostics).into_error(provider, status.into())
                );
            }
            Ok(output)
        };

        tokio::time::timeout(self.timeout, exchange)
            .await
            .map_err(|_| ProviderError::timeout(provider, self.timeout))?
    }
}

/// All of `stream`, or an error once it passes `limit` bytes.
async fn bounded(
    provider: &str,
    stream: impl AsyncRead + Unpin,
    limit: u64,
) -> Result<Vec<u8>, ProviderError> {
    let mut output = Vec::new();
    stream
        .take(limit + 1)
        .read_to_end(&mut output)
        .await
        .map_err(|error| ProviderError::malformed(provider, error))?;
    if output.len() as u64 > limit {
        return Err(ProviderError::malformed(
            provider,
            format!("the CLI wrote more than {limit} bytes"),
        ));
    }
    Ok(output)
}

/// The first `limit` bytes of `stream`, with the rest read and dropped so the
/// child never blocks on a full pipe.
async fn truncated(
    mut stream: impl AsyncRead + Unpin,
    limit: u64,
) -> Result<Vec<u8>, ProviderError> {
    let mut kept = Vec::new();
    let _ = (&mut stream).take(limit).read_to_end(&mut kept).await;
    let _ = tokio::io::copy(&mut stream, &mut tokio::io::sink()).await;
    Ok(kept)
}
