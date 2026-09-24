use crate::branch_name::BranchName;
use crate::parse_error::ParseError;
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
    /// and a path.
    #[error("Invalid API origin: expected an HTTPS URL with no query, fragment or credentials")]
    InvalidOrigin,

    /// A value passed in, or one GitHub answered with, is not what it has to
    /// be.
    #[error(transparent)]
    Parse(#[from] ParseError),
}

/// A pull request operation, or what went wrong with it.
pub type PullRequestResult<T> = Result<T, PullRequestError>;
