use crate::branch_name::BranchName;
use crate::parse_error::ParseError;
use crate::pull_request::JobLogError;
use crate::pull_request::RateLimit;
use thiserror::Error;

/// What went wrong asking GitHub about, or acting on, a pull request or a
/// repository.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PullRequestError {
    /// GitHub answered in a way no other variant names, for the reason
    /// given: an answer this crate cannot read or that is too long to, a
    /// write it redirected somewhere the write is not sent on to, or a refusal
    /// its GraphQL API reported. An HTTP status no other variant names is
    /// [`PullRequestError::Refused`].
    #[error("GitHub API error: {0}")]
    GitHubApi(String),

    /// GitHub answered with an HTTP status no other variant names: a redirect
    /// that was not followed, a conflict, a refusal of what was sent that is
    /// not a duplicate, or a failure of its own.
    #[error("GitHub refused the request with status {status}{}", said(.message))]
    #[non_exhaustive]
    Refused {
        /// The HTTP status GitHub answered with.
        status: u16,
        /// GitHub's own words about it: the message and the details its JSON
        /// answer listed, with anything shaped like a credential redacted, on
        /// one line and bounded. Empty where the answer carried neither or was
        /// not GitHub's JSON, since the raw answer never reaches an error, and
        /// never taken from the request.
        message: String,
    },

    /// GitHub did not accept the token, or a request was given an empty
    /// token without
    /// [`PullRequestService::with_anonymous_reads`](crate::pull_request::PullRequestService::with_anonymous_reads)
    /// and was refused unsent.
    #[error("Authentication failed")]
    AuthenticationFailed,

    /// A request that is not a read, meaning a write or a GraphQL call, was
    /// given an empty token under
    /// [`PullRequestService::with_anonymous_reads`](crate::pull_request::PullRequestService::with_anonymous_reads)
    /// and was refused unsent, because GitHub answers none of them without
    /// one.
    #[error("This request needs a token, and none was given")]
    AuthenticationRequired,

    /// GitHub accepted the token, but it may not do this.
    #[error("The token may not do this")]
    Forbidden,

    /// GitHub's rate limit was reached; the request can be retried later.
    #[error("GitHub's rate limit was reached")]
    RateLimited,

    /// GitHub's rate limit for the token is spent until it resets, so waiting
    /// before then will not help. Only a service built with
    /// [`PullRequestService::with_retry_after`](crate::pull_request::PullRequestService::with_retry_after)
    /// reports it, for a 403 or 429 whose `x-ratelimit-remaining` is 0. A
    /// GraphQL answer of 200 whose errors name `RATE_LIMITED` is
    /// [`PullRequestError::RateLimited`], never this.
    #[error("GitHub's rate limit is exhausted until it resets")]
    #[non_exhaustive]
    QuotaExhausted {
        /// What GitHub said of the limit, including when it resets, boxed so
        /// that every other error stays small.
        rate_limit: Box<RateLimit>,
    },

    /// What was asked for does not exist, or the token cannot see it.
    ///
    /// Where GitHub names which of a repository and its pull request it could
    /// not find, the error is [`PullRequestError::RepositoryNotFound`] or
    /// [`PullRequestError::PullRequestNotFound`] instead;
    /// [`PullRequestError::is_not_found`] is `true` for all three.
    #[error("Not found, or not visible to this token")]
    NotFound,

    /// The repository does not exist, or the token cannot see it, as GitHub
    /// said in answering for it.
    ///
    /// GitHub tells the two apart only where it names the missing part, as
    /// its GraphQL API does; elsewhere the error is
    /// [`PullRequestError::NotFound`], and a missing pull request is
    /// [`PullRequestError::PullRequestNotFound`].
    /// [`PullRequestError::is_not_found`] is `true` for all three.
    #[error("Repository not found, or not visible to this token")]
    RepositoryNotFound,

    /// The repository was found, but has no pull request with the number
    /// asked for, as GitHub said in answering for it.
    ///
    /// A repository GitHub cannot find is
    /// [`PullRequestError::RepositoryNotFound`], and anything missing where
    /// GitHub does not name which part is [`PullRequestError::NotFound`].
    /// [`PullRequestError::is_not_found`] is `true` for all three.
    #[error("The repository has no pull request with that number")]
    PullRequestNotFound,

    /// A pull request is already open from the branch.
    #[error("A pull request already exists for branch: {0}")]
    PullRequestAlreadyExists(BranchName),

    /// A repository with the name asked for already exists; the name is one
    /// this crate accepted as a repository name.
    #[error("A repository named {0} already exists")]
    RepositoryExists(String),

    /// GitHub will not merge the pull request as it stands, for the reason
    /// given.
    #[error("The pull request cannot be merged: {0}")]
    NotMergeable(String),

    /// Branch protection refused the merge, for the reason given.
    #[error("Branch protection refused: {0}")]
    Protected(String),

    /// The pull request's head moved after it was read, so it was not merged
    /// at a commit nobody had looked at; or GitHub says a branch was modified
    /// while it merged. Either way a fresh read and a retry may succeed.
    #[error("The pull request's head moved since it was read")]
    HeadMoved,

    /// The repository does not allow auto-merge, so a pull request in it
    /// cannot be set to merge once it is ready.
    #[error("The repository does not allow auto-merge")]
    AutoMergeNotAllowed,

    /// The repository does not allow the merge method asked for, for the
    /// reason given.
    #[error("The repository does not allow this merge method: {0}")]
    MergeMethodNotAllowed(String),

    /// The pull request has nothing left to wait for, so GitHub will not set it
    /// to merge once it is ready: it can be merged now.
    #[error("The pull request can be merged now")]
    AlreadyMergeable,

    /// A job's log could not be downloaded, for the reason given.
    #[error(transparent)]
    JobLog(#[from] JobLogError),

    /// The content is larger than the limit the caller set, so it was not
    /// read.
    #[error("The content is larger than the limit the caller set")]
    TooLarge,

    /// The request did not complete, or its answer could not be read.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// The URL does not name a repository on the host this service answers
    /// for.
    #[error("Invalid repository URL")]
    InvalidRepositoryUrl,

    /// A repository owner or name has characters GitHub does not allow in
    /// one. The value is not repeated back.
    #[error("Invalid repository owner or name")]
    InvalidRepositoryName,

    /// The configured API origin is not an HTTPS URL with nothing but a host
    /// and a path, nor, where plain HTTP was opted in to, an HTTP one.
    #[error(
        "Invalid API origin: expected an HTTPS (or opted-in HTTP) URL with no query, fragment or credentials"
    )]
    InvalidOrigin,

    /// A value passed in, or one GitHub answered with, is not what it has to
    /// be.
    #[error(transparent)]
    Parse(#[from] ParseError),
}

impl PullRequestError {
    /// GitHub's rate limit for the token is spent until it resets, as
    /// `rate_limit` says.
    pub fn quota_exhausted(rate_limit: RateLimit) -> Self {
        Self::QuotaExhausted {
            rate_limit: Box::new(rate_limit),
        }
    }

    /// GitHub answered with `status`, an HTTP status no other variant names,
    /// saying `message` about it.
    pub fn refused(status: u16, message: impl Into<String>) -> Self {
        Self::Refused {
            status,
            message: message.into(),
        }
    }

    /// Whether what was asked for was not found, or not visible to the token:
    /// [`PullRequestError::NotFound`],
    /// [`PullRequestError::RepositoryNotFound`] or
    /// [`PullRequestError::PullRequestNotFound`].
    pub fn is_not_found(&self) -> bool {
        matches!(
            self,
            Self::NotFound | Self::RepositoryNotFound | Self::PullRequestNotFound
        )
    }
}

/// What follows the status in a refusal's text: GitHub's `message` after a
/// colon, or nothing where it said nothing.
fn said(message: &str) -> String {
    match message.is_empty() {
        true => String::new(),
        false => format!(": {message}"),
    }
}

/// A pull request operation, or what went wrong with it.
pub type PullRequestResult<T> = Result<T, PullRequestError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_three_not_found_errors_are_not_found() {
        for found in [
            PullRequestError::NotFound,
            PullRequestError::RepositoryNotFound,
            PullRequestError::PullRequestNotFound,
        ] {
            assert!(found.is_not_found(), "{found:?}");
        }
        for other in [
            PullRequestError::Forbidden,
            PullRequestError::RateLimited,
            PullRequestError::AuthenticationFailed,
        ] {
            assert!(!other.is_not_found(), "{other:?}");
        }
    }

    #[test]
    fn a_refusal_names_its_status_and_github_s_words_only_when_there_are_any() {
        let refused = PullRequestError::refused(503, "Service Unavailable");
        assert!(
            matches!(
                refused,
                PullRequestError::Refused { status: 503, ref message } if message == "Service Unavailable"
            ),
            "{refused:?}"
        );
        assert_eq!(
            refused.to_string(),
            "GitHub refused the request with status 503: Service Unavailable"
        );
        assert_eq!(
            PullRequestError::refused(500, "").to_string(),
            "GitHub refused the request with status 500"
        );
        assert!(!PullRequestError::refused(404, "").is_not_found());
    }
}
