use thiserror::Error;

/// PR service errors
#[derive(Debug, Error)]
pub enum PrError {
    #[error("GitHub API error: {0}")]
    GitHubApi(String),

    #[error("Repository not configured")]
    NoRepository,

    #[error("Authentication failed")]
    AuthFailed,

    #[error("Branch not found: {0}")]
    BranchNotFound(String),

    #[error("PR already exists for branch: {0}")]
    PrAlreadyExists(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Invalid repository URL: {0}")]
    InvalidRepoUrl(String),
}

pub type PrResult<T> = Result<T, PrError>;
