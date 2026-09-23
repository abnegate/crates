//! What a delivery can fail with.

use std::time::Duration;

use thiserror::Error;

use crate::endpoint::EndpointError;

/// A single channel's failure.
///
/// No variant carries the endpoint URL. A webhook URL is a bearer credential,
/// and an error message is the shortest path from a credential to a log file,
/// so failures name the host and nothing more.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum NotifyError {
    #[error("delivery timed out after {}ms", .after.as_millis())]
    Timeout { after: Duration },

    #[error("the notifier panicked")]
    Panicked,

    #[error("{host} rejected the notification with HTTP {status}: {body}")]
    Rejected {
        host: String,
        status: u16,
        body: String,
    },

    #[error("{host} is rate limiting this webhook{}", retry_hint(.retry_after))]
    RateLimited {
        host: String,
        retry_after: Option<Duration>,
    },

    #[error("could not reach {host}: {message}")]
    Unreachable { host: String, message: String },

    #[error("SMTP delivery via {host} failed: {message}")]
    Smtp { host: String, message: String },

    #[error("the message could not be built: {message}")]
    Malformed { message: String },

    #[error(transparent)]
    Endpoint(#[from] EndpointError),
}

impl NotifyError {
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
        let error = NotifyError::Timeout {
            after: Duration::from_millis(2500),
        };
        assert_eq!(error.to_string(), "delivery timed out after 2500ms");
        assert!(error.is_retryable());
    }

    #[test]
    fn a_rate_limit_reports_the_wait_when_the_provider_gave_one() {
        let told = NotifyError::RateLimited {
            host: "discord.com".to_string(),
            retry_after: Some(Duration::from_millis(1200)),
        };
        assert_eq!(
            told.to_string(),
            "discord.com is rate limiting this webhook; retry after 1200ms"
        );
        assert_eq!(told.retry_after(), Some(Duration::from_millis(1200)));

        let untold = NotifyError::RateLimited {
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
        let malformed = NotifyError::Rejected {
            host: "hooks.slack.com".to_string(),
            status: 400,
            body: "invalid_payload".to_string(),
        };
        assert!(!malformed.is_retryable());

        for status in [500, 502, 503, 408, 429] {
            let transient = NotifyError::Rejected {
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
    fn a_panic_is_never_retried() {
        assert!(!NotifyError::Panicked.is_retryable());
        assert_eq!(NotifyError::Panicked.to_string(), "the notifier panicked");
    }
}
