use std::fmt;
use std::sync::LazyLock;

use abnegate_secret::REDACTED;
use chrono::DateTime;
use chrono::TimeDelta;
use chrono::Utc;
use regex::Regex;

/// What a prompt still arriving lacks when codex has printed nothing of it.
pub(crate) const NO_LINK: &str = "no sign-in link";

/// What a prompt still arriving lacks once codex has printed its link.
pub(crate) const NO_CODE: &str = "no one-time code";

/// What a prompt still arriving lacks once codex has printed its code.
pub(crate) const NO_EXPIRY: &str = "no expiry for its one-time code";

static LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(https?://[^\s]+)[ \t]*$").expect("the link pattern is a valid regex")
});
static CODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*([A-Z0-9]+(?:-[A-Z0-9]+)+)[ \t]*$")
        .expect("the code pattern is a valid regex")
});
static EXPIRY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"expires in (\d+) minutes").expect("the expiry pattern is a valid regex")
});

/// The sign-in link and one-time code `codex login --device-auth` prints for
/// a person to enter.
///
/// Whoever holds the code can finish the sign-in with their own account, so
/// `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Prompt {
    /// The page the person opens to sign in.
    pub verification_url: String,
    /// The one-time code the person enters there.
    pub user_code: String,
    /// When the code expires, as codex counted it from when it printed it.
    pub expires_at: DateTime<Utc>,
}

impl Prompt {
    /// A prompt to enter `user_code` at `verification_url` before
    /// `expires_at`.
    pub fn new(
        verification_url: impl Into<String>,
        user_code: impl Into<String>,
        expires_at: DateTime<Utc>,
    ) -> Self {
        Self {
            verification_url: verification_url.into(),
            user_code: user_code.into(),
            expires_at,
        }
    }

    /// The prompt in `text`, codex's stdout with its colour codes stripped,
    /// as read at `now`. Until all of it has arrived, the first part codex
    /// has not printed yet.
    ///
    /// Only a line after the link can be the code: codex always prints the
    /// link first.
    pub(crate) fn read(text: &str, now: DateTime<Utc>) -> Result<Self, &'static str> {
        let link = LINK.captures(text).ok_or(NO_LINK)?;
        let after = link.get(0).map_or(0, |whole| whole.end());
        let code = CODE.captures_at(text, after).ok_or(NO_CODE)?;
        let expires_at = EXPIRY
            .captures(text)
            .and_then(|expiry| expiry[1].parse::<i64>().ok())
            .and_then(TimeDelta::try_minutes)
            .and_then(|lifetime| now.checked_add_signed(lifetime))
            .ok_or(NO_EXPIRY)?;
        Ok(Self::new(&link[1], &code[1], expires_at))
    }
}

impl fmt::Debug for Prompt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Prompt")
            .field("verification_url", &self.verification_url)
            .field("user_code", &REDACTED)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::login::output::strip;
    use crate::test_support::cli::EXCHANGE_FAILED;
    use crate::test_support::cli::EXCHANGE_FAILED_PROMPT;
    use crate::test_support::cli::NO_COLOR_PROMPT;
    use crate::test_support::cli::POLL_FAILED;
    use crate::test_support::cli::POLL_FAILED_PROMPT;
    use crate::test_support::cli::PROMPT;
    use crate::test_support::cli::REFUSED;
    use crate::test_support::cli::RELOGIN_ABANDONED_PROMPT;
    use crate::test_support::cli::SUCCESS;
    use crate::test_support::cli::SUCCESS_PROMPT;
    use crate::test_support::cli::TERM_DUMB_PROMPT;
    use crate::test_support::cli::THROTTLED;
    use crate::test_support::cli::UNSUPPORTED;

    fn stripped(raw: &[u8]) -> String {
        String::from_utf8_lossy(raw)
            .lines()
            .map(|line| format!("{}\n", strip(line)))
            .collect()
    }

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_790_148_196, 0).expect("a valid timestamp")
    }

    #[test]
    fn the_link_code_and_expiry_are_read_from_codexs_own_prompt() {
        let prompt = Prompt::read(&stripped(PROMPT), now()).expect("a whole prompt");

        assert_eq!(
            prompt.verification_url,
            "https://auth.openai.com/codex/device"
        );
        assert_eq!(prompt.user_code, "ABCD-EFGHI");
        assert_eq!(prompt.expires_at, now() + TimeDelta::minutes(15));
    }

    #[test]
    fn every_recorded_prompt_reads_the_same_whatever_the_terminal() {
        for raw in [
            SUCCESS_PROMPT,
            NO_COLOR_PROMPT,
            TERM_DUMB_PROMPT,
            POLL_FAILED_PROMPT,
            EXCHANGE_FAILED_PROMPT,
            RELOGIN_ABANDONED_PROMPT,
        ] {
            let prompt = Prompt::read(&stripped(raw), now()).expect("a whole prompt");

            assert!(
                prompt.verification_url.starts_with("http://127.0.0.1:")
                    && prompt.verification_url.ends_with("/codex/device"),
                "not the fake issuer's link: {}",
                prompt.verification_url
            );
            assert_eq!(prompt.user_code, "FXTR-9Z9Z9");
            assert_eq!(prompt.expires_at, now() + TimeDelta::minutes(15));
        }
    }

    #[test]
    fn colour_codes_are_stripped_from_the_link() {
        let raw = String::from_utf8_lossy(PROMPT);
        let line = raw
            .lines()
            .find(|line| line.contains("https://"))
            .expect("the link line");

        assert!(line.contains('\u{1b}'), "the capture lost its colour codes");
        assert_eq!(strip(line), "   https://auth.openai.com/codex/device");
    }

    #[test]
    fn a_prompt_still_arriving_names_the_part_that_is_missing() {
        let full = stripped(PROMPT);
        let through = |needle: &str| {
            let end = full.find(needle).expect("the needle") + needle.len();
            full[..end].to_string()
        };

        assert_eq!(Prompt::read("", now()), Err(NO_LINK));
        assert_eq!(
            Prompt::read(&through("codex/device\n"), now()),
            Err(NO_CODE)
        );
        assert_eq!(
            Prompt::read(
                &full.replace("expires in 15 minutes", "expires soon"),
                now()
            ),
            Err(NO_EXPIRY)
        );
        assert_eq!(
            Prompt::read(
                "   ABCD-EFGHI\n   https://auth.openai.com/codex/device\n",
                now()
            ),
            Err(NO_CODE),
            "a code printed before the link was taken for the code"
        );
    }

    #[test]
    fn nothing_else_codex_prints_reads_as_a_prompt() {
        for raw in [
            SUCCESS,
            REFUSED,
            UNSUPPORTED,
            THROTTLED,
            POLL_FAILED,
            EXCHANGE_FAILED,
        ] {
            assert_eq!(
                Prompt::read(&stripped(raw), now()),
                Err(NO_LINK),
                "{}",
                String::from_utf8_lossy(raw)
            );
        }
    }

    #[test]
    fn an_expiry_too_large_to_represent_is_no_expiry() {
        let full = stripped(PROMPT).replace(
            "expires in 15 minutes",
            "expires in 99999999999999999 minutes",
        );

        assert_eq!(Prompt::read(&full, now()), Err(NO_EXPIRY));
    }

    #[test]
    fn the_code_never_reaches_debug_output() {
        let prompt = Prompt::read(&stripped(PROMPT), now()).expect("a whole prompt");

        let rendered = format!("{prompt:?}");

        assert!(!rendered.contains("ABCD-EFGHI"), "{rendered}");
        assert!(rendered.contains(REDACTED), "{rendered}");
    }
}
