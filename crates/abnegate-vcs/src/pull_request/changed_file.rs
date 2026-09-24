use crate::pull_request::FileStatus;
use serde::Deserialize;

/// One file a pull request changes, and by how much.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChangedFile {
    /// Its path in the repository, after any rename.
    pub filename: String,
    /// What the pull request does to it, or [`FileStatus::Unknown`] when
    /// GitHub does not say.
    #[serde(default)]
    pub status: FileStatus,
    /// Lines it adds.
    pub additions: u32,
    /// Lines it removes.
    pub deletions: u32,
}
