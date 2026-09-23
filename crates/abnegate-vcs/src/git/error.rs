use crate::branch_name::BranchName;
use crate::parse_error::ParseError;
use std::path::PathBuf;
use thiserror::Error;

/// What went wrong running git.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GitError {
    #[error("Git command failed: {0}")]
    CommandFailed(String),

    #[error("Git operation timed out")]
    TimedOut,

    #[error("Remote not configured")]
    NoRemote,

    #[error("No changes to commit")]
    NoChanges,

    #[error("Branch already exists: {0}")]
    BranchExists(BranchName),

    #[error(transparent)]
    Parse(#[from] ParseError),

    #[error("Refusing to run in a repository whose configuration sets {0:?}")]
    UnsafeConfig(String),

    #[error("Refusing to remove directory outside worktrees area: {}", .0.display())]
    UnsafeWorktree(PathBuf),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type GitResult<T> = Result<T, GitError>;
