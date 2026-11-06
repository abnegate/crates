use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::kind::AgentKind;
use crate::login::LoginError;
use crate::login::claude::Plan;
use crate::login::output::Output;
use crate::login::process::Process;

const CLAUDE: &[&str] = &["auth", "status"];
const CODEX: &[&str] = &["login", "status"];
const LOGGED_IN: &str = "loggedIn";
const SUBSCRIPTION: &str = "subscriptionType";
const SIGNED_IN: &str = "Logged in using ";
const SIGNED_OUT: &str = "Not logged in";
const DETAIL: &str = " - ";
const ARTICLES: &[&str] = &["an ", "a "];

/// Whether an agent's CLI says it is signed in, as `claude auth status` or
/// `codex login status` reports it.
///
/// Neither CLI checks its credentials to answer: signed in means credentials
/// are present.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Status {
    /// Whether the CLI holds a sign-in.
    pub signed_in: bool,
    /// How the CLI says it is signed in, when it is: a Claude plan as the
    /// claude CLI shows it, such as `Claude Team`, or how codex signed in,
    /// such as `ChatGPT` or `API key`, never with any of the key.
    pub label: Option<String>,
}

impl Status {
    /// A CLI that is signed in, or is not, with no label.
    pub fn new(signed_in: bool) -> Self {
        Self {
            signed_in,
            label: None,
        }
    }

    /// This status, labelled `label`.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Asks `agent`'s CLI at `executable` whether it is signed in.
    /// `environment` is the CLI's whole environment, so it decides which
    /// home the CLI reads; the CLI runs in its own process group with
    /// nothing on its stdin, and is stopped if it does not answer in time.
    ///
    /// # Errors
    ///
    /// [`LoginError::Unavailable`] when the CLI cannot be started,
    /// [`LoginError::Failed`] in its own words when it fails for any reason
    /// other than being signed out, such as a corrupt login, and
    /// [`LoginError::Unreadable`] when it prints a status this cannot read or
    /// does not finish in time.
    pub async fn check(
        agent: AgentKind,
        executable: &Path,
        environment: &BTreeMap<String, String>,
    ) -> Result<Self, LoginError> {
        let arguments = match agent {
            AgentKind::Claude => CLAUDE,
            AgentKind::Codex => CODEX,
        };
        let output = Output::capture(
            Process::command(executable, arguments, environment),
            executable,
        )
        .await?;
        match agent {
            AgentKind::Claude => claude(&output, executable),
            AgentKind::Codex => codex(&output, executable),
        }
    }
}

/// `claude auth status` prints its JSON whether or not it is signed in, and
/// exits 1 when it is not.
fn claude(output: &Output, executable: &Path) -> Result<Status, LoginError> {
    let status = match serde_json::from_str::<Value>(&output.stdout) {
        Ok(status) => status,
        Err(_) if !output.status.success() => return Err(output.failed(executable)),
        Err(error) => {
            return Err(LoginError::Unreadable {
                reason: format!(
                    "{} printed a status that cannot be read: {error}",
                    executable.display()
                ),
            });
        }
    };
    let signed_in = status
        .get(LOGGED_IN)
        .and_then(Value::as_bool)
        .ok_or_else(|| LoginError::Unreadable {
            reason: format!(
                "{} printed a status without {LOGGED_IN}",
                executable.display()
            ),
        })?;
    let label = status
        .get(SUBSCRIPTION)
        .and_then(Value::as_str)
        .filter(|_| signed_in)
        .map(str::trim)
        .and_then(Plan::named)
        .map(Plan::label);
    Ok(labelled(signed_in, label))
}

/// `codex login status` also exits 1 for a corrupt login or a missing home,
/// so only its own words tell signed out from broken.
fn codex(output: &Output, executable: &Path) -> Result<Status, LoginError> {
    match output.status.code() {
        Some(0) => Ok(labelled(true, method(&output.stderr))),
        Some(1) if output.stderr.contains(SIGNED_OUT) => Ok(Status::new(false)),
        _ => Err(output.failed(executable)),
    }
}

fn labelled(signed_in: bool, label: Option<&str>) -> Status {
    let status = Status::new(signed_in);
    match label {
        Some(label) => status.with_label(label),
        None => status,
    }
}

/// How codex says it is signed in, without the fragment of the key it shows
/// for an API key.
fn method(stderr: &str) -> Option<&str> {
    let method = stderr
        .lines()
        .find_map(|line| line.trim().strip_prefix(SIGNED_IN))?;
    let method = method
        .split_once(DETAIL)
        .map_or(method, |(method, _)| method);
    let method = ARTICLES
        .iter()
        .find_map(|article| method.strip_prefix(*article))
        .unwrap_or(method)
        .trim();
    (!method.is_empty()).then_some(method)
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    use tempfile::TempDir;

    use super::*;
    use crate::test_support::cli::capture;
    use crate::test_support::cli::environment;
    use crate::test_support::cli::fake;
    use crate::test_support::cli::fixture;
    use crate::test_support::cli::recorded;

    const CLAUDE_SIGNED_IN: &[u8] = fixture!("claude-auth-status-signed-in.json");
    const CLAUDE_SIGNED_OUT: &[u8] = fixture!("claude-auth-status-signed-out.json");
    const CLAUDE_TOKEN: &[u8] = fixture!("claude-auth-status-oauth-token.json");
    const CLAUDE_KEY: &[u8] = fixture!("claude-auth-status-api-key.json");
    const CODEX_SIGNED_OUT: &[u8] = fixture!("codex-login-status-signed-out.stderr");
    const CODEX_CHATGPT: &[u8] = fixture!("fake-issuer/codex-login-status-chatgpt.stderr");
    const CODEX_KEY: &[u8] = fixture!("fake-issuer/codex-login-status-api-key.stderr");
    const CODEX_LOGGED_OUT: &[u8] = fixture!("fake-issuer/codex-login-status-after-logout.stderr");
    const CODEX_CORRUPT: &[u8] = fixture!("fake-issuer/codex-login-status-corrupt-auth.stderr");
    const CODEX_UNCONFIGURED: &[u8] =
        fixture!("fake-issuer/codex-login-status-missing-home.stderr");
    const MISSING: &str = "/nonexistent/cli";

    fn exited(code: i32) -> ExitStatus {
        ExitStatus::from_raw(code << 8)
    }

    fn output(code: i32, stdout: &[u8], stderr: &[u8]) -> Output {
        Output {
            status: exited(code),
            stdout: String::from_utf8_lossy(stdout).into_owned(),
            stderr: String::from_utf8_lossy(stderr).into_owned(),
        }
    }

    fn cli() -> &'static Path {
        Path::new("/usr/local/bin/cli")
    }

    fn signed_in(label: Option<&str>) -> Status {
        labelled(true, label)
    }

    fn signed_out() -> Status {
        Status::new(false)
    }

    #[test]
    fn claude_signed_in_to_claude_ai_is_labelled_with_its_plan() {
        let status = claude(&output(0, CLAUDE_SIGNED_IN, b""), cli()).expect("claude's status");

        assert_eq!(status, signed_in(Some("Claude Team")));
        assert!(
            String::from_utf8_lossy(CLAUDE_SIGNED_IN).contains(r#""subscriptionType": "team""#)
        );
    }

    #[test]
    fn a_claude_plan_the_cli_does_not_name_has_no_label() {
        let printed = String::from_utf8_lossy(CLAUDE_SIGNED_IN).replace(r#""team""#, r#""free""#);

        let status = claude(&output(0, printed.as_bytes(), b""), cli()).expect("claude's status");

        assert_eq!(status, signed_in(None));
    }

    #[test]
    fn claude_signed_out_has_no_label() {
        let status = claude(&output(1, CLAUDE_SIGNED_OUT, b""), cli()).expect("claude's status");

        assert_eq!(status, signed_out());
    }

    #[test]
    fn claude_signed_in_with_a_token_or_a_key_has_no_plan_to_show() {
        for printed in [CLAUDE_TOKEN, CLAUDE_KEY] {
            let status = claude(&output(0, printed, b""), cli()).expect("claude's status");

            assert_eq!(status, signed_in(None));
        }
    }

    #[test]
    fn claude_output_that_is_not_its_status_is_an_error() {
        let crashed = claude(&output(2, b"", b"TypeError: undefined\n"), cli());
        assert!(
            matches!(&crashed, Err(LoginError::Failed { reason, .. }) if reason == "TypeError: undefined"),
            "{crashed:?}"
        );

        let garbled = claude(&output(0, b"Logged in\n", b""), cli());
        assert!(
            matches!(garbled, Err(LoginError::Unreadable { .. })),
            "{garbled:?}"
        );

        let incomplete = claude(&output(0, b"{\"authMethod\":\"none\"}", b""), cli());
        assert!(
            matches!(incomplete, Err(LoginError::Unreadable { .. })),
            "{incomplete:?}"
        );
    }

    #[test]
    fn codex_signed_in_with_chatgpt_is_labelled_chatgpt() {
        let status = codex(&output(0, b"", CODEX_CHATGPT), cli()).expect("codex's status");

        assert_eq!(status, signed_in(Some("ChatGPT")));
    }

    #[test]
    fn codex_signed_in_with_a_key_is_never_labelled_with_the_key() {
        let status = codex(&output(0, b"", CODEX_KEY), cli()).expect("codex's status");

        assert_eq!(status, signed_in(Some("API key")));
        assert!(String::from_utf8_lossy(CODEX_KEY).contains("sk-"));
    }

    #[test]
    fn codex_not_logged_in_is_signed_out() {
        for printed in [CODEX_SIGNED_OUT, CODEX_LOGGED_OUT] {
            let status = codex(&output(1, b"", printed), cli()).expect("codex's status");

            assert_eq!(status, signed_out());
        }
    }

    #[test]
    fn codex_failing_for_any_other_reason_is_an_error_not_signed_out() {
        for printed in [CODEX_CORRUPT, CODEX_UNCONFIGURED] {
            let status = codex(&output(1, b"", printed), cli());

            let expected = String::from_utf8_lossy(printed).trim().to_string();
            assert!(
                matches!(&status, Err(LoginError::Failed { reason, .. }) if *reason == expected),
                "{status:?}"
            );
        }
    }

    #[tokio::test]
    async fn each_cli_is_asked_with_exactly_the_given_environment() {
        for (agent, arguments, stdout, stderr, expected) in [
            (
                AgentKind::Claude,
                "auth status",
                CLAUDE_SIGNED_IN,
                &b""[..],
                signed_in(Some("Claude Team")),
            ),
            (
                AgentKind::Codex,
                "login status",
                &b""[..],
                CODEX_CHATGPT,
                signed_in(Some("ChatGPT")),
            ),
        ] {
            let directory = TempDir::new().expect("a temporary directory");
            let record = directory.path().join("environment");
            let printed = capture(&directory, "stdout", stdout);
            let said = capture(&directory, "stderr", stderr);
            let executable = fake(
                &directory,
                arguments,
                &format!(
                    "env > '{}'\ncat '{printed}'\ncat '{said}' >&2\nexit 0",
                    record.display()
                ),
            );
            let mut given = environment();
            given.insert("HOST_STATUS_TEST".to_string(), "kept".to_string());

            let status = Status::check(agent, &executable, &given)
                .await
                .expect("the CLI's status");

            assert_eq!(status, expected);
            assert!(
                std::env::var_os("CARGO_MANIFEST_DIR").is_some(),
                "the test runner no longer sets CARGO_MANIFEST_DIR, so its absence below proves nothing"
            );
            assert_eq!(recorded(&record), given);
        }
    }

    #[tokio::test]
    async fn a_missing_cli_is_unavailable() {
        for agent in AgentKind::ALL {
            let error = Status::check(*agent, Path::new(MISSING), &environment())
                .await
                .expect_err("a missing CLI");

            assert!(
                matches!(&error, LoginError::Unavailable { executable, .. } if executable == Path::new(MISSING)),
                "{error:?}"
            );
        }
    }
}
