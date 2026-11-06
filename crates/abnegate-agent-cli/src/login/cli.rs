use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::path::PathBuf;

use abnegate_secret::REDACTED;
use abnegate_secret::SecretValue;
use reqwest::Url;

use crate::login::LoginError;

/// An agent's CLI as a sign-in step runs it: its executable, and exactly the
/// environment it is given, nothing of this process's own.
///
/// The environment comes in two parts. [Variables](Self::with_environment)
/// are settings such as `PATH`. A [secret](Self::with_secret), such as a
/// proxy URL that carries a password or an API key, is set the same way,
/// wins over a variable of the same name, and has its value scrubbed from the
/// reason of every [`LoginError`] the step returns: as it was given and, when
/// it is a URL, as the URL normalised and its password, encoded or decoded.
///
/// `Debug` names every variable and shows none of their values.
///
/// ```
/// use std::collections::BTreeMap;
///
/// use abnegate_agent_cli::login::Cli;
/// use abnegate_secret::SecretValue;
///
/// let cli = Cli::new("/usr/local/bin/codex")
///     .with_environment(BTreeMap::from([("PATH".to_string(), "/usr/bin:/bin".to_string())]))
///     .with_secret("HTTPS_PROXY", SecretValue::new("http://user:hunter2@proxy.example:8080"));
///
/// assert!(!format!("{cli:?}").contains("hunter2"));
/// ```
#[derive(Clone)]
pub struct Cli {
    executable: PathBuf,
    environment: BTreeMap<String, String>,
    secrets: BTreeMap<String, SecretValue>,
}

impl Cli {
    /// The CLI at `executable`, run with an empty environment.
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            environment: BTreeMap::new(),
            secrets: BTreeMap::new(),
        }
    }

    /// This CLI, with `environment` as the variables it is given in place of
    /// any it was given before. Its secrets are kept.
    pub fn with_environment(mut self, environment: BTreeMap<String, String>) -> Self {
        self.environment = environment;
        self
    }

    /// This CLI, also given `variable` set to the secret `value`, which is
    /// scrubbed from every reason a step that runs it fails with.
    pub fn with_secret(
        mut self,
        variable: impl Into<String>,
        value: impl Into<SecretValue>,
    ) -> Self {
        self.secrets.insert(variable.into(), value.into());
        self
    }

    /// The executable that is run.
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Every variable the CLI is given, each secret over a variable of the
    /// same name.
    pub(crate) fn variables(&self) -> impl Iterator<Item = (&str, &str)> {
        self.environment
            .iter()
            .filter(|(variable, _)| !self.secrets.contains_key(*variable))
            .map(|(variable, value)| (variable.as_str(), value.as_str()))
            .chain(
                self.secrets
                    .iter()
                    .map(|(variable, value)| (variable.as_str(), value.expose())),
            )
    }

    /// `error` with every secret this CLI is given scrubbed from its reason.
    pub(crate) fn scrubbed(&self, error: LoginError) -> LoginError {
        match error {
            LoginError::Failed { reason } => LoginError::Failed {
                reason: self.scrub(reason),
            },
            LoginError::Unreadable { reason } => LoginError::Unreadable {
                reason: self.scrub(reason),
            },
            LoginError::Unavailable { executable, reason } => LoginError::Unavailable {
                executable,
                reason: self.scrub(reason),
            },
            error => error,
        }
    }

    /// `text` with every form of every secret this CLI is given redacted, the
    /// longest first, so no part of one survives in place of a shorter one
    /// inside it.
    pub(crate) fn scrub(&self, text: String) -> String {
        let mut secrets = self
            .secrets
            .values()
            .flat_map(|secret| forms(secret.expose()))
            .filter(|secret| !secret.is_empty())
            .collect::<Vec<_>>();
        secrets.sort_by(|one, other| other.len().cmp(&one.len()).then_with(|| one.cmp(other)));
        secrets.dedup();
        secrets
            .into_iter()
            .fold(text, |text, secret| text.replace(&secret, REDACTED))
    }
}

/// `secret` as given, and when it is a URL with a host, as a CLI that parsed
/// it would repeat it: normalised, with and without a trailing slash, and its
/// password alone, percent-encoded and decoded.
fn forms(secret: &str) -> Vec<String> {
    let mut forms = vec![secret.to_string()];
    let Ok(url) = Url::parse(secret) else {
        return forms;
    };
    if !url.has_host() {
        return forms;
    }
    let normalised = url.as_str();
    forms.push(normalised.to_string());
    forms.push(normalised.trim_end_matches('/').to_string());
    if let Some(password) = url.password() {
        forms.push(password.to_string());
        if let Ok(decoded) = urlencoding::decode(password) {
            forms.push(decoded.into_owned());
        }
    }
    forms
}

impl fmt::Debug for Cli {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Cli")
            .field("executable", &self.executable)
            .field("environment", &self.environment.keys().collect::<Vec<_>>())
            .field("secrets", &self.secrets.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "hunter2";
    const PROXY: &str = "http://user:hunter2@proxy.example:8080";

    fn cli() -> Cli {
        Cli::new("/usr/local/bin/codex")
            .with_environment(BTreeMap::from([
                ("PATH".to_string(), "/usr/bin:/bin".to_string()),
                (
                    "HTTPS_PROXY".to_string(),
                    "http://proxy.example:8080".to_string(),
                ),
            ]))
            .with_secret("HTTPS_PROXY", PROXY)
            .with_secret("OPENAI_API_KEY", "sk-fake-key")
            .with_secret("EMPTY", "")
    }

    #[test]
    fn debug_names_every_variable_without_its_value() {
        let rendered = format!("{:?}", cli());

        for name in ["PATH", "HTTPS_PROXY", "OPENAI_API_KEY"] {
            assert!(rendered.contains(name), "{name}: {rendered}");
        }
        for value in [PASSWORD, "/usr/bin:/bin", "sk-fake-key"] {
            assert!(!rendered.contains(value), "{value}: {rendered}");
        }
    }

    #[test]
    fn a_secret_wins_over_a_variable_of_the_same_name() {
        let cli = cli();
        let variables = cli.variables().collect::<BTreeMap<_, _>>();

        assert_eq!(
            variables,
            BTreeMap::from([
                ("EMPTY", ""),
                ("HTTPS_PROXY", PROXY),
                ("OPENAI_API_KEY", "sk-fake-key"),
                ("PATH", "/usr/bin:/bin"),
            ])
        );
    }

    #[test]
    fn every_reason_is_scrubbed_of_every_secret() {
        let cli = cli().with_secret("SHORTER", PASSWORD);
        let said = format!("could not reach {PROXY} with sk-fake-key");
        for error in [
            LoginError::Failed {
                reason: said.clone(),
            },
            LoginError::Unreadable {
                reason: said.clone(),
            },
            LoginError::Unavailable {
                executable: PathBuf::from("/usr/local/bin/codex"),
                reason: said.clone(),
            },
        ] {
            let scrubbed = cli.scrubbed(error);

            let shown = format!("{scrubbed} {scrubbed:?}");
            for secret in [PASSWORD, "proxy.example", "sk-fake-key"] {
                assert!(!shown.contains(secret), "{secret}: {shown}");
            }
            assert!(shown.contains("could not reach [REDACTED]"), "{shown}");
        }
    }

    #[test]
    fn a_secret_url_is_scrubbed_however_the_cli_repeats_it() {
        let cli = Cli::new("/usr/local/bin/codex")
            .with_secret("HTTPS_PROXY", "HTTP://user:p%40ss-word@Proxy.Example:8080");
        let said = "proxy http://user:p%40ss-word@proxy.example:8080/ refused, \
                    as did http://user:p%40ss-word@proxy.example:8080 for user p@ss-word";

        let scrubbed = cli.scrub(said.to_string());

        for secret in ["p%40ss-word", "p@ss-word", "proxy.example"] {
            assert!(!scrubbed.contains(secret), "{secret}: {scrubbed}");
        }
    }
}
