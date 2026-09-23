use thiserror::Error;

/// What went wrong running git.
#[derive(Debug, Error)]
pub enum GitError {
    #[error("Git command failed: {0}")]
    CommandFailed(String),

    #[error("Repository not found at {0}")]
    RepositoryNotFound(String),

    #[error("Remote not configured")]
    NoRemote,

    #[error("No changes to commit")]
    NoChanges,

    #[error("Branch already exists: {0}")]
    BranchExists(String),

    #[error("Authentication failed")]
    AuthenticationFailed,

    #[error("Invalid {label} (contains disallowed characters): {value}")]
    InvalidReference { label: String, value: String },

    #[error("Refusing to remove directory outside worktrees area: {0}")]
    UnsafeWorktree(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type GitResult<T> = Result<T, GitError>;
