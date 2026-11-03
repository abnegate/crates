use chrono::DateTime;
use chrono::Utc;

use crate::provider::window::Window;

/// The wordings, compared without regard to case, in which a coding agent
/// says a usage limit refused its turn.
///
/// `hit your` is deliberately bare: claude refuses in words that need not name
/// a limit at all, such as "You've hit your team's shared budget". It also
/// matches prose that is no refusal, such as "the retry hit your webhook", so
/// only a coding agent's own report of a failed turn is worth reading with
/// [`Limit::worded`], never an endpoint's error or arbitrary output.
pub const LIMIT_WORDINGS: &[&str] = &[
    "hit your",
    "session limit",
    "usage limit",
    "weekly limit",
    "rate limit",
];

/// A turn a usage limit refused.
///
/// `message` is the refusal in the agent's own words, which is what
/// [`ProviderError::Limited`] renders, so text that classifies a failure reads
/// a limit exactly as it read the agent failure it replaces.
///
/// [`ProviderError::Limited`]: crate::provider::ProviderError::Limited
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Limit {
    /// The refusal, in the agent's own words.
    pub message: String,
    /// When the limit lifts, when the agent said.
    pub resets_at: Option<DateTime<Utc>>,
    /// Whether the account ran out of usage credits rather than plan usage.
    pub credits: bool,
    /// The window that refused the turn, when the agent said which.
    pub window: Option<Window>,
}

impl Limit {
    /// A refusal saying `message`, with nothing else known about it.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            resets_at: None,
            credits: false,
            window: None,
        }
    }

    /// Whether `words` say, in any of [`LIMIT_WORDINGS`], that a usage limit
    /// refused the turn.
    pub fn worded(words: &str) -> bool {
        let lowered = words.to_lowercase();
        LIMIT_WORDINGS
            .iter()
            .any(|wording| lowered.contains(wording))
    }

    /// The same refusal, saying `message` instead.
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    /// The same refusal, lifting at `resets_at`.
    pub fn with_resets_at(mut self, resets_at: DateTime<Utc>) -> Self {
        self.resets_at = Some(resets_at);
        self
    }

    /// The same refusal, marked as the account's usage credits running out
    /// when `credits` holds.
    pub fn with_credits(mut self, credits: bool) -> Self {
        self.credits = credits;
        self
    }

    /// The same refusal, made by `window`.
    pub fn with_window(mut self, window: Window) -> Self {
        self.window = Some(window);
        self
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

    use super::Limit;
    use crate::provider::window::Window;

    #[test]
    fn a_limit_is_worded_in_any_case_and_a_sign_in_failure_is_not() {
        for words in [
            "You've hit your session limit · resets 5pm",
            "You have hit your USAGE LIMIT. Try again later.",
            "Rate limit reached for requests",
            "You've reached your weekly limit",
        ] {
            assert!(Limit::worded(words), "{words}");
        }
        for words in [
            "Not logged in · Please run /login",
            "Invalid API key provided",
            "stream disconnected before completion",
        ] {
            assert!(!Limit::worded(words), "{words}");
        }
    }

    #[test]
    fn a_refusal_that_names_no_limit_is_still_worded_as_one() {
        for words in [
            "You've hit your limit · resets 5pm",
            "You've hit your Opus limit · resets Mon 9am",
            "You've hit your monthly spend limit. Switch to another model to continue.",
            "You've hit your team's shared budget. Switch to another model to continue.",
        ] {
            assert!(Limit::worded(words), "{words}");
        }
    }

    #[test]
    fn words_near_a_limit_that_use_no_wording_are_not_a_limit() {
        for words in [
            "maximum context length exceeded",
            "limit reached for this file",
            "you have reached the end of the stream",
            "the session ended",
            "this rate is limited to admins",
        ] {
            assert!(!Limit::worded(words), "{words}");
        }
    }

    #[test]
    fn hit_your_is_deliberately_broad() {
        assert!(Limit::worded("the retry hit your webhook"));
    }

    #[test]
    fn every_builder_sets_its_own_field() {
        let resets_at = DateTime::from_timestamp(1_790_208_000, 0).expect("a valid timestamp");
        let window = Window::new("five_hour").with_used_percent(100.0);

        let limit = Limit::new("You've hit your session limit")
            .with_message("rate limit reached (five_hour, rejected): You've hit your session limit")
            .with_resets_at(resets_at)
            .with_credits(true)
            .with_window(window.clone());

        assert_eq!(
            limit.message,
            "rate limit reached (five_hour, rejected): You've hit your session limit"
        );
        assert_eq!(limit.resets_at, Some(resets_at));
        assert!(limit.credits);
        assert_eq!(limit.window, Some(window));
    }

    #[test]
    fn a_new_limit_knows_only_its_message() {
        let limit = Limit::new("usage limit");

        assert_eq!(limit.message, "usage limit");
        assert_eq!(limit.resets_at, None);
        assert!(!limit.credits);
        assert_eq!(limit.window, None);
    }
}
