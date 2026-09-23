//! Keeping the secrets a run was given out of everything it writes down.

use std::borrow::Cow;
use std::sync::Arc;

use abnegate_secret::REDACTED;
use abnegate_secret::SecretValue;
use abnegate_secret::redact;

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
    /// A scrubber for every value in `secrets`, which should be everything
    /// the run's child was handed that is not public.
    pub(crate) fn new(secrets: impl IntoIterator<Item = SecretValue>) -> Self {
        let mut secrets: Vec<SecretValue> = secrets
            .into_iter()
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
    use abnegate_secret::SecretValue;

    use super::Scrubber;

    fn scrubber(secrets: &[&str]) -> Scrubber {
        Scrubber::new(secrets.iter().map(|secret| SecretValue::new(*secret)))
    }

    #[test]
    fn a_secret_that_does_not_look_like_one_is_still_removed() {
        let scrubber = scrubber(&["correct horse battery", "hunter2hunter2", "plainpassword1"]);

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
        assert_eq!(
            scrubber(&["1"]).scrub("exited with 1 error"),
            "exited with 1 error"
        );
    }

    #[test]
    fn debug_never_prints_a_secret() {
        assert!(!format!("{:?}", scrubber(&["hunter2hunter2"])).contains("hunter2"));
    }
}
