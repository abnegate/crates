use thiserror::Error;

/// A value refused before it could reach a git command line.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseError {
    #[error("Invalid branch name: {0:?}")]
    BranchName(String),

    #[error("Invalid commit identifier: {0:?}")]
    CommitSha(String),

    #[error("Expected an HTTPS GitHub owner/repository URL")]
    RepositoryUrl,
}
