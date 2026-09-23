use crate::branch_name::BranchName;
use crate::parse_error::ParseError;
use thiserror::Error;

/// What went wrong opening or reading back a pull request.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PullRequestError {
    #[error("GitHub API error: {0}")]
    GitHubApi(String),

    #[error("Authentication failed")]
    AuthenticationFailed,

    #[error("The token may not do this")]
    Forbidden,

    #[error("GitHub's rate limit was reached")]
    RateLimited,

    #[error("Not found, or not visible to this token")]
    NotFound,

    #[error("A pull request already exists for branch: {0}")]
    PullRequestAlreadyExists(BranchName),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Invalid repository URL")]
    InvalidRepositoryUrl,

    #[error("Invalid API origin: expected an HTTPS URL with no query, fragment or credentials")]
    InvalidOrigin,

    #[error(transparent)]
    Parse(#[from] ParseError),
}

pub type PullRequestResult<T> = Result<T, PullRequestError>;
