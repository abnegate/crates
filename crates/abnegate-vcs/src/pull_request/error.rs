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
    /// GitHub refused the request, or answered in a way no other variant
    /// names, for the reason given.
    #[error("GitHub API error: {0}")]
    GitHubApi(String),

    /// GitHub did not accept the token.
    #[error("Authentication failed")]
    AuthenticationFailed,

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
        /// What GitHub said of the limit, including when it resets.
        rate_limit: RateLimit,
    },

    /// What was asked for does not exist, or the token cannot see it.
    #[error("Not found, or not visible to this token")]
    NotFound,

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
        Self::QuotaExhausted { rate_limit }
    }
}

/// A pull request operation, or what went wrong with it.
pub type PullRequestResult<T> = Result<T, PullRequestError>;
