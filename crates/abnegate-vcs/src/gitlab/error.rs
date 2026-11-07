use crate::parse_error::ParseError;
use std::time::Duration;
use thiserror::Error;

/// What went wrong reading a project, its tree or a blob from GitLab.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GitLabError {
    /// GitLab answered in a way no other variant names, for the fixed reason
    /// given: an answer this crate cannot read, a JSON answer too long to
    /// read, a link to a tree's next page that repeats one already followed,
    /// or a page of a tree with no entries that still links to a next one.
    /// The answer itself is never quoted.
    #[error("GitLab API error: {0}")]
    GitLabApi(String),

    /// GitLab answered with an HTTP status no other variant names, such as a
    /// redirect that was not followed or a 5xx failure of its own.
    #[error("GitLab answered with status {status}")]
    #[non_exhaustive]
    Refused {
        /// The HTTP status GitLab answered with.
        status: u16,
    },

    /// GitLab did not accept the token, with a 401; or a request was refused
    /// unsent, given an empty token without
    /// [`GitLabService::with_anonymous_reads`](crate::gitlab::GitLabService::with_anonymous_reads)
    /// or a token that cannot be sent as a header value. The token is never
    /// repeated back.
    #[error("Authentication failed")]
    AuthenticationFailed,

    /// GitLab accepted the token, but it may not do this.
    #[error("The token may not do this")]
    Forbidden,

    /// What was asked for does not exist, or the token cannot see it.
    #[error("Not found, or not visible to this token")]
    NotFound,

    /// GitLab's rate limit was reached, with a 429; the request can be
    /// retried later.
    #[error("GitLab's rate limit was reached")]
    #[non_exhaustive]
    RateLimited {
        /// How long GitLab asked to be left alone before the request is sent
        /// again, read from its `Retry-After` header, or `None` when it named
        /// no wait this crate can read.
        retry_after: Option<Duration>,
    },

    /// The blob is larger than the limit the caller set, so it was not
    /// returned.
    #[error("The content is larger than the limit the caller set")]
    TooLarge,

    /// The request did not complete, or its answer could not be read.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// The configured URL is not an HTTPS URL with nothing but a host and a
    /// path, nor, where plain HTTP was opted in to, an HTTP one; or it names
    /// a link-local address or a cloud provider's metadata service.
    #[error(
        "Invalid GitLab origin: expected an HTTPS (or opted-in HTTP) URL with no query, fragment or credentials, outside link-local and metadata addresses"
    )]
    InvalidOrigin,

    /// A value passed in, or one GitLab answered with, is not what it has to
    /// be.
    #[error(transparent)]
    Parse(#[from] ParseError),
}

impl GitLabError {
    /// GitLab answered with `status`, an HTTP status no other variant names.
    pub fn refused(status: u16) -> Self {
        Self::Refused { status }
    }

    /// GitLab's rate limit was reached, and it asked for a wait of
    /// `retry_after` before the request is sent again, if it named one.
    pub fn rate_limited(retry_after: Option<Duration>) -> Self {
        Self::RateLimited { retry_after }
    }
}

/// A GitLab read, or what went wrong with it.
pub type GitLabResult<T> = Result<T, GitLabError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_names_its_status() {
        let refused = GitLabError::refused(503);

        assert!(
            matches!(refused, GitLabError::Refused { status: 503 }),
            "{refused:?}"
        );
        assert_eq!(refused.to_string(), "GitLab answered with status 503");
    }

    #[test]
    fn a_rate_limit_names_the_wait_gitlab_asked_for() {
        let limited = GitLabError::rate_limited(Some(Duration::from_secs(30)));

        assert!(
            matches!(
                limited,
                GitLabError::RateLimited { retry_after: Some(wait) } if wait == Duration::from_secs(30)
            ),
            "{limited:?}"
        );
        assert_eq!(limited.to_string(), "GitLab's rate limit was reached");
    }
}
