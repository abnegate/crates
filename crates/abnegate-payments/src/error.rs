//! What talking to Stripe, or verifying a webhook, can fail with.

use abnegate_secret::sanitize;
use stripe::StripeError;
use stripe_webhook::WebhookError;

/// The result of every fallible operation in this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong creating a session or verifying a webhook.
///
/// No variant carries a secret key, a webhook secret, or a request URL. A
/// variant may gain a field in a minor release, so a [`Payments`](crate::Payments)
/// implemented outside this crate, a test double included, fails through a
/// constructor rather than a literal. A pattern outside this crate ends in `..`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The webhook signature did not match the body, or the signing secret
    /// could not be used.
    #[error("the webhook signature is not valid")]
    Unverified,

    /// The webhook signature's timestamp is outside Stripe's replay window.
    #[error("the webhook signature is outside the allowed time window")]
    Stale,

    /// The body, a request, or a client could not be built from what the
    /// caller supplied.
    #[error("the payment request could not be built: {message}")]
    #[non_exhaustive]
    Invalid {
        /// What could not be built, never quoting a secret.
        message: String,
    },

    /// Stripe answered, and refused the request.
    #[error("Stripe refused the request with HTTP {status}")]
    #[non_exhaustive]
    Refused {
        /// The HTTP status Stripe answered with.
        status: u16,
    },

    /// The request never reached Stripe, Stripe's answer never arrived, or the
    /// attempt timed out.
    #[error("could not reach Stripe: {message}")]
    #[non_exhaustive]
    Unreachable {
        /// The transport's reason, with secrets and request URLs removed.
        message: String,
    },

    /// Stripe returned a Checkout Session without a hosted URL.
    #[error("Stripe returned a checkout session without a URL")]
    Incomplete,

    /// A webhook body could not be read as a Stripe event.
    #[error("the webhook body could not be read")]
    Malformed,
}

impl Error {
    /// The webhook signature did not match, or the signing secret could not
    /// be used.
    pub fn unverified() -> Self {
        Self::Unverified
    }

    /// The webhook signature's timestamp is outside Stripe's replay window.
    pub fn stale() -> Self {
        Self::Stale
    }

    /// A request, a client or a body could not be built, as `message` says.
    /// It is sanitized, and must not quote a secret.
    pub fn invalid(message: impl std::fmt::Display) -> Self {
        Self::Invalid {
            message: sanitize(&message.to_string()).into_owned(),
        }
    }

    /// Stripe answered with `status` rather than success.
    pub fn refused(status: u16) -> Self {
        Self::Refused { status }
    }

    /// The request never reached Stripe, or its answer never arrived, for
    /// `message`. It is sanitized, and must not carry a secret or a URL.
    pub fn unreachable(message: impl std::fmt::Display) -> Self {
        Self::Unreachable {
            message: sanitize(&message.to_string()).into_owned(),
        }
    }

    /// Stripe returned a Checkout Session without a hosted URL.
    pub fn incomplete() -> Self {
        Self::Incomplete
    }

    /// A webhook body could not be read as a Stripe event.
    pub fn malformed() -> Self {
        Self::Malformed
    }

    pub(crate) fn from_stripe(error: StripeError) -> Self {
        match error {
            StripeError::Stripe(_, status) => Self::refused(status),
            StripeError::Timeout => Self::unreachable("timeout communicating with Stripe"),
            StripeError::ConfigError(_) => {
                Self::invalid("the Stripe client could not be built from the given secret")
            }
            StripeError::JSONDeserialize(_) => {
                Self::unreachable("Stripe returned a body that could not be read")
            }
            StripeError::ClientError(message) => {
                let lower = message.to_ascii_lowercase();
                if lower.contains("secret") || lower.contains("header") {
                    Self::invalid("the Stripe client could not be built from the given secret")
                } else {
                    Self::unreachable(message)
                }
            }
        }
    }

    pub(crate) fn from_webhook(error: WebhookError) -> Self {
        match error {
            WebhookError::BadKey | WebhookError::BadSignature => Self::unverified(),
            WebhookError::BadTimestamp(_) => Self::stale(),
            WebhookError::BadHeader(_) | WebhookError::BadParse(_) => Self::malformed(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_constructor_fills_its_variant() {
        assert!(matches!(Error::unverified(), Error::Unverified));
        assert!(matches!(Error::stale(), Error::Stale));
        assert!(matches!(Error::incomplete(), Error::Incomplete));
        assert!(matches!(Error::malformed(), Error::Malformed));
        assert!(
            matches!(Error::refused(402), Error::Refused { status: 402, .. }),
            "the status is kept"
        );
        assert!(
            matches!(
                Error::invalid("no line items"),
                Error::Invalid { message } if message == "no line items"
            ),
            "the message is kept"
        );
        assert!(
            matches!(
                Error::unreachable("connection refused"),
                Error::Unreachable { message } if message == "connection refused"
            ),
            "the message is kept"
        );
    }

    #[test]
    fn constructors_sanitize_the_text_they_are_given() {
        let secret = "sk_live_hunter2seventeen";
        let invalid = Error::invalid(format!("client for {secret}"));
        let unreachable = Error::unreachable(format!("posted {secret} to https://api.stripe.com"));
        for error in [&invalid, &unreachable] {
            let rendered = error.to_string();
            assert!(!rendered.contains(secret), "{rendered}");
            assert!(!rendered.contains("hunter2"), "{rendered}");
        }
    }

    #[test]
    fn a_config_error_does_not_repeat_the_secret() {
        let error = Error::from_stripe(StripeError::ConfigError(
            "secret can only include visible ASCII characters".into(),
        ));
        let rendered = format!("{error} {error:?}");
        assert!(
            !rendered.to_lowercase().contains("ascii"),
            "{rendered} must not quote Stripe's config reason, which names the secret"
        );
        assert!(matches!(error, Error::Invalid { .. }));
    }

    #[test]
    fn a_client_error_that_names_the_secret_field_is_invalid() {
        let error = Error::from_stripe(StripeError::ClientError(
            "`secret` can only include visible ASCII characters".into(),
        ));
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains("ASCII"), "{rendered}");
        assert!(matches!(error, Error::Invalid { .. }));
    }
}
