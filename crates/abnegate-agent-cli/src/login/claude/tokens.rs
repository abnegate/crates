use std::time::Duration;

use abnegate_secret::SecretValue;
use chrono::DateTime;
use chrono::TimeDelta;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use crate::login::claude::Plan;

/// A Claude sign-in's tokens.
///
/// `Debug` hides both tokens. `Serialize` writes them **in plaintext**, as
/// [`SecretValue`] does, in a JSON object whose shape is part of this type's
/// contract:
/// `{"access", "refresh", "expires_at", "issued_at", "scope", "subscription"}`,
/// the times in RFC 3339. Seal what it writes before it reaches storage;
/// `Deserialize` reads it back, and an object without `issued_at` reads with
/// it unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Tokens {
    /// The access token the claude CLI is run with.
    pub access: SecretValue,
    /// The token that renews `access`, when Claude granted one.
    pub refresh: Option<SecretValue>,
    /// When `access` expires.
    pub expires_at: DateTime<Utc>,
    /// When Claude granted `access`, when known.
    #[serde(default)]
    pub issued_at: Option<DateTime<Utc>>,
    /// The scopes Claude granted, space-separated.
    pub scope: String,
    /// The plan the account is on, as Claude spells it, when it said.
    pub subscription: Option<String>,
}

impl Tokens {
    /// Tokens as Claude granted them.
    pub fn new(
        access: SecretValue,
        refresh: Option<SecretValue>,
        expires_at: DateTime<Utc>,
        issued_at: Option<DateTime<Utc>>,
        scope: impl Into<String>,
        subscription: Option<String>,
    ) -> Self {
        Self {
            access,
            refresh,
            expires_at,
            issued_at,
            scope: scope.into(),
            subscription,
        }
    }

    /// Whether `access` expires within `margin` of `now`, or already has.
    pub fn expiring(&self, now: DateTime<Utc>, margin: Duration) -> bool {
        let margin = TimeDelta::from_std(margin).unwrap_or(TimeDelta::MAX);
        self.expires_at.signed_duration_since(now) <= margin
    }

    /// How long `access` was granted for: `None` when the time of the grant
    /// is unknown, or `access` expires before it.
    pub fn lifetime(&self) -> Option<Duration> {
        self.issued_at.and_then(|issued_at| {
            self.expires_at
                .signed_duration_since(issued_at)
                .to_std()
                .ok()
        })
    }

    /// The plan `subscription` names, when it is one the claude CLI knows.
    pub fn plan(&self) -> Option<Plan> {
        self.subscription.as_deref().and_then(Plan::named)
    }
}

#[cfg(test)]
mod tests {
    use abnegate_secret::REDACTED;

    use super::*;

    const ACCESS: &str = "fake-access-token-for-tests";
    const REFRESH: &str = "fake-refresh-token-for-tests";
    const LIFETIME: Duration = Duration::from_secs(8 * 60 * 60);
    const SHAPE: &str = r#"{"access":"fake-access-token-for-tests","refresh":"fake-refresh-token-for-tests","expires_at":"2026-09-21T14:13:20Z","issued_at":"2026-09-21T06:13:20Z","scope":"user:inference","subscription":"max"}"#;
    const UNTIMED: &str = r#"{"access":"fake-access-token-for-tests","refresh":"fake-refresh-token-for-tests","expires_at":"2026-09-21T14:13:20Z","scope":"user:inference","subscription":"max"}"#;

    fn expiry() -> DateTime<Utc> {
        DateTime::from_timestamp(1_790_000_000, 0).expect("a valid timestamp")
    }

    fn tokens() -> Tokens {
        Tokens::new(
            SecretValue::new(ACCESS),
            Some(SecretValue::new(REFRESH)),
            expiry(),
            Some(expiry() - TimeDelta::hours(8)),
            "user:inference",
            Some("max".to_string()),
        )
    }

    #[test]
    fn tokens_serialise_to_the_pinned_shape_and_back() {
        let written = serde_json::to_string(&tokens()).expect("serialise");

        assert_eq!(written, SHAPE);
        assert_eq!(
            serde_json::from_str::<Tokens>(SHAPE).expect("deserialise"),
            tokens()
        );
    }

    #[test]
    fn tokens_written_before_their_lifetime_was_recorded_read_with_it_unknown() {
        let read = serde_json::from_str::<Tokens>(UNTIMED).expect("the earlier shape");

        assert_eq!(
            read,
            Tokens {
                issued_at: None,
                ..tokens()
            }
        );
        assert_eq!(read.lifetime(), None);
    }

    #[test]
    fn the_lifetime_runs_from_the_grant_to_the_expiry() {
        assert_eq!(tokens().lifetime(), Some(LIFETIME));
        let backwards = Tokens {
            issued_at: Some(expiry() + TimeDelta::seconds(1)),
            ..tokens()
        };
        assert_eq!(backwards.lifetime(), None);
    }

    #[test]
    fn debug_of_tokens_is_redacted() {
        let rendered = format!("{:?}", tokens());

        assert!(
            !rendered.contains(ACCESS) && !rendered.contains(REFRESH),
            "{rendered}"
        );
        assert!(rendered.contains(REDACTED), "{rendered}");
    }

    #[test]
    fn tokens_are_expiring_once_inside_the_margin() {
        let tokens = tokens();
        let margin = Duration::from_secs(5 * 60);
        let delta = TimeDelta::minutes(5);

        assert!(!tokens.expiring(expiry() - TimeDelta::minutes(6), margin));
        assert!(tokens.expiring(expiry() - delta, margin));
        assert!(tokens.expiring(expiry() + TimeDelta::minutes(1), margin));
        assert!(tokens.expiring(DateTime::<Utc>::MIN_UTC, Duration::MAX));
    }

    #[test]
    fn the_plan_is_named_as_the_cli_names_it() {
        let mut tokens = tokens();

        for (subscription, label) in [
            ("max", Some("Claude Max")),
            ("pro", Some("Claude Pro")),
            ("team", Some("Claude Team")),
            ("enterprise", Some("Claude Enterprise")),
            ("free", None),
        ] {
            tokens.subscription = Some(subscription.to_string());
            assert_eq!(tokens.plan().map(Plan::label), label, "{subscription}");
        }
        tokens.subscription = None;
        assert_eq!(tokens.plan(), None);
    }
}
