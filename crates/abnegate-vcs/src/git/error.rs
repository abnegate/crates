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

    /// A branch the caller named that is a symbolic ref.
    #[error("Refusing to write the branch {0} through a symbolic ref")]
    SymbolicBranch(BranchName),

    /// [`GitError::SymbolicBranch`] for the checked-out branch, whose name
    /// the repository chose and which is left unnamed.
    #[error("Refusing to write the checked-out branch through a symbolic ref")]
    SymbolicHead,

    /// [`GitError::SymbolicBranch`] for the remote's default branch, whose
    /// name the repository chose and which is left unnamed.
    #[error("Refusing a default branch that is a symbolic ref")]
    SymbolicDefaultBranch,

    #[error(transparent)]
    Parse(#[from] ParseError),

    #[error("Refusing to run in a repository whose configuration sets {0:?}")]
    UnsafeConfig(String),

    /// A symbolic link somewhere under the repository's git directory, which
    /// git would write through. Where it stands is left unnamed: the
    /// repository chose that name.
    #[error("Refusing a repository whose git directory holds a symbolic link")]
    LinkedPath,

    /// A checkout whose git directory, or the one it shares, is not the one
    /// its own `.git` names: git would read and write another repository's
    /// refs, index and configuration from it. Which one is left unnamed: the
    /// checkout chose that path.
    #[error("Refusing a checkout whose git directory is not its own")]
    RedirectedGitDirectory,

    #[error("Refusing to remove directory outside worktrees area: {}", .0.display())]
    UnsafeWorktree(PathBuf),

    /// A nested repository standing where the index records a gitlink. Its
    /// path is the one the index holds, which a run can choose, so it is
    /// shown escaped.
    #[error("Refusing to stage beside the nested repository at {0:?}")]
    NestedRepository(PathBuf),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// A git operation, or what went wrong with it.
pub type GitResult<T> = Result<T, GitError>;
