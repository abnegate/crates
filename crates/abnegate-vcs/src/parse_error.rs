use thiserror::Error;

/// A value refused before it could reach a git command line or a request.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseError {
    /// Not a branch name git accepts, or one a command line could misread.
    #[error("Invalid branch name: {0:?}")]
    BranchName(String),

    /// Not a full commit identifier.
    #[error("Invalid commit identifier: {0:?}")]
    CommitSha(String),

    /// Not a full git object name.
    #[error("Invalid object name: {0:?}")]
    ObjectId(String),

    /// Not a path in a repository: empty, or with an empty or dot segment, or
    /// a control character.
    #[error("Invalid repository path: {0:?}")]
    RepositoryPath(String),

    /// Not an HTTPS GitHub owner/repository URL.
    #[error("Expected an HTTPS GitHub owner/repository URL")]
    RepositoryUrl,

    /// Not a tag name git accepts: one `git check-ref-format` refuses under
    /// `refs/tags/`, or that `git tag` refuses for being `HEAD` or for being
    /// led by `-`.
    #[error("Invalid release tag: {0:?}")]
    Tag(String),
}
