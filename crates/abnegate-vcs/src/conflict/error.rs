use crate::parse_error::ParseError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConflictError {
    #[error("Git command failed: {0}")]
    CommandFailed(String),

    #[error(transparent)]
    Parse(#[from] ParseError),

    #[error("Unsafe conflicted path: {0}")]
    UnsafePath(String),

    #[error("{branch} is at {actual}, not the expected {expected}")]
    Moved {
        branch: String,
        expected: String,
        actual: String,
    },

    #[error("The branch merges cleanly; there is no conflict to repair")]
    NoConflict,

    #[error("Conflicted file {0} carries no conflict markers and cannot be repaired as text")]
    NotTextual(String),

    #[error("The checkout no longer holds the conflicted state it was prepared with")]
    CheckoutMoved,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type ConflictResult<T> = Result<T, ConflictError>;
