use crate::branch_name::BranchName;
use crate::commit_sha::CommitSha;
use crate::git::GitError;
use crate::parse_error::ParseError;
use thiserror::Error;

/// What went wrong reproducing, repairing or publishing a conflict.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConflictError {
    #[error("Git command failed: {0}")]
    CommandFailed(String),

    #[error(transparent)]
    Git(#[from] GitError),

    #[error(transparent)]
    Parse(#[from] ParseError),

    #[error("Unsafe conflicted path: {0}")]
    UnsafePath(String),

    #[error("{branch} is at {actual}, not the expected {expected}")]
    Moved {
        branch: BranchName,
        expected: CommitSha,
        actual: CommitSha,
    },

    #[error("The branch merges cleanly; there is no conflict to repair")]
    NoConflict,

    #[error("Conflicted file {0} carries no conflict markers and cannot be repaired as text")]
    NotTextual(String),

    #[error("The checkout no longer holds the conflicted state it was prepared with")]
    CheckoutMoved,

    #[error("The repair touched files outside the conflict: {}", .0.join(", "))]
    Strays(Vec<String>),

    #[error("The checkout's HEAD is not the applied resolution of this conflict")]
    NotApplied,

    #[error("The push was rejected; the branch moved while it was being repaired")]
    Rejected,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type ConflictResult<T> = Result<T, ConflictError>;
