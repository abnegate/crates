//! Keeping the secrets a run was given out of everything it writes down.

use std::borrow::Cow;
use std::sync::Arc;

use abnegate_secret::REDACTED;
use abnegate_secret::SecretValue;
use abnegate_secret::redact;

use crate::settings::CliSettings;

/// Shorter values are too likely to occur by chance for replacing every
/// occurrence to be worth what it destroys.
const MINIMUM: usize = 8;

/// Redacts credential-shaped text, and every secret value this run was
/// handed, from text on its way to a log, an error or a caller.
///
/// Pattern redaction alone misses a secret that does not look like one, such
/// as a password, and an agent is as likely to echo that as anything else.
#[derive(Debug, Clone, Default)]
pub(crate) struct Scrubber {
    secrets: Arc<[SecretValue]>,
}

impl Scrubber {
    pub(crate) fn new(settings: &CliSettings) -> Self {
        let servers = settings.mcp.servers.values();
        let mut secrets: Vec<SecretValue> = settings
            .credential
            .expose()
            .map(SecretValue::new)
            .into_iter()
            .chain(settings.environment.values().cloned())
            .chain(
                servers
                    .flat_map(|server| server.environment.values().chain(server.headers.values()))
                    .cloned(),
            )
            .filter(|secret| secret.expose().len() >= MINIMUM)
            .collect();
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.expose().len()));
        secrets.dedup();
        Self {
            secrets: secrets.into(),
        }
    }

    pub(crate) fn scrub<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut text = redact(text);
        for secret in self.secrets.iter() {
            if text.contains(secret.expose()) {
                text = Cow::Owned(text.replace(secret.expose(), REDACTED));
            }
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use abnegate_llm::Credential;
    use abnegate_secret::SecretValue;

    use super::Scrubber;
    use crate::mcp::McpServer;
    use crate::settings::CliSettings;

    #[test]
    fn a_secret_that_does_not_look_like_one_is_still_removed() {
        let settings = CliSettings::default()
            .with_credential(Credential::key(
                "DATABASE_PASSWORD",
                "correct horse battery",
            ))
            .with_environment("SERVICE_PASSWORD", "hunter2hunter2")
            .with_mcp_server(
                "grafana",
                McpServer {
                    command: Some("uvx".to_string()),
                    headers: [(
                        "Authorization".to_string(),
                        SecretValue::new("plainpassword1"),
                    )]
                    .into(),
                    ..McpServer::default()
                },
            );
        let scrubber = Scrubber::new(&settings);

        let scrubbed = scrubber.scrub(
            "login failed for correct horse battery, then hunter2hunter2, then plainpassword1",
        );
        assert_eq!(
            scrubbed,
            "login failed for [REDACTED], then [REDACTED], then [REDACTED]"
        );
    }

    #[test]
    fn credential_shaped_text_is_redacted_even_when_it_was_never_configured() {
        let scrubbed = Scrubber::default()
            .scrub("fatal: bad key sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        assert!(!scrubbed.contains("sk-ant-api03-AAAA"), "{scrubbed}");
    }

    #[test]
    fn a_short_value_is_left_alone() {
        let settings = CliSettings::default().with_environment("VERBOSE", "1");
        assert_eq!(
            Scrubber::new(&settings).scrub("exited with 1 error"),
            "exited with 1 error"
        );
    }

    #[test]
    fn debug_never_prints_a_secret() {
        let settings = CliSettings::default().with_environment("TOKEN", "hunter2hunter2");
        assert!(!format!("{:?}", Scrubber::new(&settings)).contains("hunter2"));
    }
}
