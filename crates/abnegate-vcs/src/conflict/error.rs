use crate::branch_name::BranchName;
use crate::commit_sha::CommitSha;
use crate::git::GitError;
use crate::parse_error::ParseError;
use thiserror::Error;

/// What went wrong reproducing, repairing or publishing a conflict.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConflictError {
    /// A git command failed. The text names the operation, never git's own
    /// output, which can quote paths and remote messages.
    #[error("Git command failed: {0}")]
    CommandFailed(String),

    /// The git service underneath failed.
    #[error(transparent)]
    Git(#[from] GitError),

    /// A branch name, commit or path from git did not parse.
    #[error(transparent)]
    Parse(#[from] ParseError),

    /// A path is empty, reaches outside the checkout, or is not one of the
    /// conflicted files as a regular file.
    #[error("Unsafe conflicted path: {0}")]
    UnsafePath(String),

    /// A branch is not at the commit the caller expected.
    #[error("{branch} is at {actual}, not the expected {expected}")]
    #[non_exhaustive]
    Moved {
        /// The branch that moved.
        branch: BranchName,
        /// The commit the caller expected it at.
        expected: CommitSha,
        /// The commit it was found at.
        actual: CommitSha,
    },

    /// The branch merges cleanly, so there is nothing to repair.
    #[error("The branch merges cleanly; there is no conflict to repair")]
    NoConflict,

    /// A conflicted file is not UTF-8 text with conflict markers, so it
    /// cannot be repaired as text.
    #[error("Conflicted file {0} carries no conflict markers and cannot be repaired as text")]
    NotTextual(String),

    /// A repaired file still carries conflict markers.
    #[error("Conflicted file {0} still carries conflict markers")]
    MarkersRemain(String),

    /// The checkout no longer holds the merge it was prepared with.
    #[error("The checkout no longer holds the conflicted state it was prepared with")]
    CheckoutMoved,

    /// The repair changed files the conflict did not name.
    #[error("The repair touched files outside the conflict: {}", .0.join(", "))]
    Strays(Vec<String>),

    /// The checkout's HEAD is not the merge of the reproduced head and base.
    #[error("The checkout's HEAD is not the applied resolution of this conflict")]
    NotApplied,

    /// The push was not a fast-forward: the branch moved during the repair.
    #[error("The push was rejected; the branch moved while it was being repaired")]
    Rejected,

    /// A local file operation failed.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// A conflict operation, or what went wrong with it.
pub type ConflictResult<T> = Result<T, ConflictError>;
