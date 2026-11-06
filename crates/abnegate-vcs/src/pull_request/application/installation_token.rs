use crate::pull_request::application::ApplicationError;
use crate::pull_request::application::service::parsed;
use crate::pull_request::application::timestamp::instant;
use abnegate_secret::SecretValue;
use serde::Deserialize;
use std::time::Duration;
use std::time::SystemTime;

/// An installation access token, and when GitHub said it expires.
#[derive(Debug, Clone)]
pub struct InstallationToken {
    secret: SecretValue,
    expires_at: SystemTime,
}

/// GitHub's answer to an installation token request, as it arrives.
#[derive(Deserialize)]
struct Payload {
    token: SecretValue,
    expires_at: String,
}

impl InstallationToken {
    /// How long before it expires a token stops being handed out.
    ///
    /// GitHub issues an installation token with an hour of life. Presenting
    /// one in its last seconds races the round trip and both clocks, and the
    /// request that loses fails as a `401` part way through someone's work,
    /// so the last five minutes count as already spent.
    pub const SAFETY_MARGIN: Duration = Duration::from_mins(5);

    /// `secret`, which GitHub said expires at `expires_at`.
    pub fn new(secret: SecretValue, expires_at: SystemTime) -> Self {
        Self { secret, expires_at }
    }

    /// GitHub's answer to an installation token request: its `token`, which
    /// is never empty, and its RFC 3339 `expires_at`.
    ///
    /// Any failure is [`ApplicationError::Response`] alone, since the answer
    /// that failed to parse is the one that may hold the token.
    pub(super) fn parse(body: &[u8]) -> Result<Self, ApplicationError> {
        let payload: Payload = parsed(body)?;
        if payload.token.expose().is_empty() {
            return Err(ApplicationError::Response);
        }
        let expires_at = instant(&payload.expires_at).ok_or(ApplicationError::Response)?;
        Ok(Self::new(payload.token, expires_at))
    }

    /// The token, to send as a bearer credential.
    pub fn secret(&self) -> &SecretValue {
        &self.secret
    }

    /// When GitHub said the token expires.
    pub fn expires_at(&self) -> SystemTime {
        self.expires_at
    }

    /// Whether the token can still be used at `now`: whether it expires
    /// more than [`Self::SAFETY_MARGIN`] after `now`. A `now` too late for
    /// the margin to be added to it finds no token usable.
    pub fn usable_at(&self, now: SystemTime) -> bool {
        now.checked_add(Self::SAFETY_MARGIN)
            .is_some_and(|spent| spent < self.expires_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::application::fixtures::at;
    use std::time::UNIX_EPOCH;

    /// An instant well after the Unix epoch.
    const NOW: u64 = 1_700_000_000;

    /// One minute.
    const MINUTE: Duration = Duration::from_mins(1);

    fn expiring_in(minutes: u32) -> InstallationToken {
        InstallationToken::new(
            SecretValue::new("ghs_installationtokenvalue"),
            at(NOW) + MINUTE * minutes,
        )
    }

    #[test]
    fn a_fresh_token_is_usable() {
        assert!(expiring_in(60).usable_at(at(NOW)));
    }

    #[test]
    fn a_token_inside_the_safety_margin_is_spent() {
        assert!(
            expiring_in(6).usable_at(at(NOW)),
            "six minutes of life is outside the margin"
        );
        assert!(
            !expiring_in(4).usable_at(at(NOW)),
            "four minutes of life is inside the margin and must not be used"
        );
        assert!(
            !expiring_in(5).usable_at(at(NOW)),
            "the margin itself is not usable time"
        );
    }

    #[test]
    fn an_expired_token_is_spent() {
        let expired = InstallationToken::new(
            SecretValue::new("ghs_installationtokenvalue"),
            at(NOW) - MINUTE,
        );

        assert!(!expired.usable_at(at(NOW)));
    }

    #[test]
    fn times_at_either_end_of_the_clock_are_judged_without_panicking() {
        let before = UNIX_EPOCH - MINUTE * 120;
        let token = InstallationToken::new(SecretValue::new("ghs_early"), UNIX_EPOCH - MINUTE * 60);

        assert!(token.usable_at(before));
        assert!(!token.usable_at(UNIX_EPOCH));

        let latest = (0..u64::BITS)
            .rev()
            .map(|bit| Duration::from_secs(1 << bit))
            .fold(UNIX_EPOCH, |reached, step| {
                reached.checked_add(step).unwrap_or(reached)
            });
        assert!(
            !InstallationToken::new(SecretValue::new("ghs_late"), latest).usable_at(latest),
            "a now the margin cannot be added to has no usable token"
        );
    }

    #[test]
    fn a_response_body_parses_into_a_token_and_an_expiry() {
        let token = InstallationToken::parse(
            br#"{"token":"ghs_installationtokenvalue","expires_at":"2026-01-15T12:00:00Z"}"#,
        )
        .unwrap();

        assert_eq!(token.secret().expose(), "ghs_installationtokenvalue");
        assert_eq!(token.expires_at(), at(1_768_478_400));
    }

    #[test]
    fn an_expiry_with_an_offset_or_a_fraction_names_the_same_instant() {
        let token = InstallationToken::parse(
            br#"{"token":"ghs_installationtokenvalue","expires_at":"2026-01-15T13:00:00.250+01:00"}"#,
        )
        .unwrap();

        assert_eq!(
            token.expires_at(),
            at(1_768_478_400) + Duration::from_millis(250)
        );
    }

    #[test]
    fn a_malformed_response_does_not_echo_the_body() {
        for body in [
            br#"{"token":"ghs_leaky","expires_at":"soon"}"#.as_slice(),
            br#"{"token":"ghs_leaky"}"#,
            br#"{"token":"ghs_leaky","#,
        ] {
            let error = InstallationToken::parse(body).expect_err("an unreadable answer fails");

            assert!(matches!(error, ApplicationError::Response));
            assert!(!error.to_string().contains("ghs_leaky"));
        }
    }

    #[test]
    fn an_empty_token_is_an_unreadable_response() {
        let error =
            InstallationToken::parse(br#"{"token":"","expires_at":"2026-01-15T12:00:00Z"}"#)
                .expect_err("an empty token authenticates nothing");

        assert!(matches!(error, ApplicationError::Response));
    }

    #[test]
    fn the_token_never_renders_itself() {
        let rendered = format!("{:?}", expiring_in(60));

        assert!(!rendered.contains("ghs_installationtokenvalue"));
        assert!(rendered.contains("[REDACTED]"));
    }
}
