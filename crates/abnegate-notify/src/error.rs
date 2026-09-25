//! What a delivery can fail with.

use std::fmt;
use std::time::Duration;

use abnegate_secret::sanitize;

use crate::endpoint::EndpointError;
use crate::text::truncate;

/// The most characters of a provider's answer that [`Error::Rejected`] keeps.
pub(crate) const MAXIMUM_ERROR_BODY_CHARACTERS: usize = 512;

/// What building a channel, or delivering through one, can fail with.
///
/// No variant carries the endpoint URL. A webhook URL is a bearer credential,
/// and an error message is the shortest path from a credential to a log file,
/// so failures name the host and nothing more.
///
/// A variant may gain a field in a minor release, so a [`Notifier`](crate::Notifier)
/// builds the error it fails with through a constructor ([`Error::timeout`],
/// [`Error::rejected`], [`Error::unreachable`] or [`Error::malformed`]), which
/// sanitizes the text it is given, and a pattern outside this crate ends in
/// `..`.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The channel did not finish within its budget.
    #[error("delivery timed out after {}ms", .after.as_millis())]
    #[non_exhaustive]
    Timeout {
        /// The budget the channel was given.
        after: Duration,
    },

    /// The channel's task panicked, and the panic went no further.
    #[error("the notifier panicked")]
    Panicked,

    /// The provider answered with a status that is neither success nor a rate
    /// limit, redirects included.
    #[error("{host} rejected the notification with HTTP {status}: {body}")]
    #[non_exhaustive]
    Rejected {
        /// The provider's host, which is safe to log.
        host: String,
        /// The HTTP status it answered with.
        status: u16,
        /// The start of its answer, bounded and sanitized.
        body: String,
    },

    /// The provider answered `429 Too Many Requests`.
    #[error("{host} is rate limiting this webhook{}", retry_hint(.retry_after))]
    #[non_exhaustive]
    RateLimited {
        /// The provider's host, which is safe to log.
        host: String,
        /// How long the provider asked the caller to wait, when it said.
        retry_after: Option<Duration>,
    },

    /// The request never reached the provider, or its answer never arrived.
    #[error("could not reach {host}: {message}")]
    #[non_exhaustive]
    Unreachable {
        /// The provider's host, which is safe to log.
        host: String,
        /// The transport's reason, with the request URL removed.
        message: String,
    },

    /// The SMTP relay could not be reached, or refused the message.
    #[error("SMTP delivery via {host} failed: {message}")]
    #[non_exhaustive]
    Smtp {
        /// The relay's host, which is safe to log.
        host: String,
        /// The relay's reason, sanitized.
        message: String,
    },

    /// A message, an address or a client could not be built from what the
    /// caller supplied.
    #[error("the message could not be built: {message}")]
    #[non_exhaustive]
    Malformed {
        /// What could not be built, never quoting an address back.
        message: String,
    },

    /// The webhook URL was refused before anything was sent to it.
    #[error(transparent)]
    Endpoint(#[from] EndpointError),
}

impl Error {
    /// The channel did not finish within `after`.
    pub fn timeout(after: Duration) -> Self {
        Self::Timeout { after }
    }

    /// `host` answered with `status`, which is neither success nor a rate
    /// limit. `body` is sanitized and cut to its start.
    pub fn rejected(host: &str, status: u16, body: &str) -> Self {
        Self::Rejected {
            host: sanitize(host).into_owned(),
            status,
            body: truncate(sanitize(body.trim()).trim(), MAXIMUM_ERROR_BODY_CHARACTERS),
        }
    }

    /// The request never reached `host`, or its answer never arrived, for
    /// `message`. It is sanitized, and must not carry the request URL.
    pub fn unreachable(host: &str, message: impl fmt::Display) -> Self {
        Self::Unreachable {
            host: sanitize(host).into_owned(),
            message: sanitize(&message.to_string()).into_owned(),
        }
    }

    /// A message, an address or a client could not be built from what the
    /// caller supplied, as `message` says. It is sanitized, and must not
    /// quote an address back.
    pub fn malformed(message: impl fmt::Display) -> Self {
        Self::Malformed {
            message: sanitize(&message.to_string()).into_owned(),
        }
    }

    /// Whether sending the same notification again could plausibly succeed.
    ///
    /// A rejected payload and an unusable endpoint will fail identically no
    /// matter how often they are retried; a timeout, a rate limit and a
    /// transport fault will not.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Timeout { .. } | Self::RateLimited { .. } | Self::Unreachable { .. } => true,
            Self::Rejected { status, .. } => *status >= 500 || *status == 408 || *status == 429,
            Self::Smtp { .. } => true,
            Self::Panicked | Self::Malformed { .. } | Self::Endpoint(_) => false,
        }
    }

    /// How long the provider asked the caller to wait, when it said.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

fn retry_hint(retry_after: &Option<Duration>) -> String {
    match retry_after {
        Some(after) => format!("; retry after {}ms", after.as_millis()),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timeout_reports_how_long_it_waited() {
        let error = Error::Timeout {
            after: Duration::from_millis(2500),
        };
        assert_eq!(error.to_string(), "delivery timed out after 2500ms");
        assert!(error.is_retryable());
    }

    #[test]
    fn a_rate_limit_reports_the_wait_when_the_provider_gave_one() {
        let told = Error::RateLimited {
            host: "discord.com".to_string(),
            retry_after: Some(Duration::from_millis(1200)),
        };
        assert_eq!(
            told.to_string(),
            "discord.com is rate limiting this webhook; retry after 1200ms"
        );
        assert_eq!(told.retry_after(), Some(Duration::from_millis(1200)));

        let untold = Error::RateLimited {
            host: "discord.com".to_string(),
            retry_after: None,
        };
        assert_eq!(
            untold.to_string(),
            "discord.com is rate limiting this webhook"
        );
        assert_eq!(untold.retry_after(), None);
    }

    #[test]
    fn client_rejections_are_permanent_and_server_rejections_are_not() {
        let malformed = Error::Rejected {
            host: "hooks.slack.com".to_string(),
            status: 400,
            body: "invalid_payload".to_string(),
        };
        assert!(!malformed.is_retryable());

        for status in [500, 502, 503, 408, 429] {
            let transient = Error::Rejected {
                host: "hooks.slack.com".to_string(),
                status,
                body: String::new(),
            };
            assert!(
                transient.is_retryable(),
                "HTTP {status} should be retryable"
            );
        }
    }

    #[test]
    fn each_constructor_fills_its_variant() {
        assert_eq!(
            Error::timeout(Duration::from_secs(2)),
            Error::Timeout {
                after: Duration::from_secs(2)
            }
        );
        assert_eq!(
            Error::rejected("hooks.slack.com", 404, "no_service"),
            Error::Rejected {
                host: "hooks.slack.com".to_string(),
                status: 404,
                body: "no_service".to_string(),
            }
        );
        assert_eq!(
            Error::unreachable("database", "connection refused"),
            Error::Unreachable {
                host: "database".to_string(),
                message: "connection refused".to_string(),
            }
        );
        assert_eq!(
            Error::malformed("no recipient"),
            Error::Malformed {
                message: "no recipient".to_string(),
            }
        );
    }

    #[test]
    fn constructors_sanitize_the_text_they_are_given() {
        let secret = "hunter2seventeen";
        let url = format!("postgres://app:{secret}@db.internal/app");

        let unreachable = Error::unreachable("database", format!("cannot connect to {url}"));
        let malformed = Error::malformed(format!("no client for {url}"));
        let rejected = Error::rejected(
            "hooks.slack.com",
            500,
            &format!("\u{1b}[31m{url} {}", "x".repeat(4_096)),
        );

        for error in [&unreachable, &malformed, &rejected] {
            assert!(!error.to_string().contains(secret), "{error}");
        }
        let Error::Rejected { body, .. } = &rejected else {
            panic!("expected a rejection, got {rejected:?}");
        };
        assert!(!body.contains('\u{1b}'), "{body:?}");
        assert!(body.chars().count() <= MAXIMUM_ERROR_BODY_CHARACTERS);
    }

    #[test]
    fn a_panic_is_never_retried() {
        assert!(!Error::Panicked.is_retryable());
        assert_eq!(Error::Panicked.to_string(), "the notifier panicked");
    }
}
