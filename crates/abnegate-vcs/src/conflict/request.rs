use crate::branch_name::BranchName;
use crate::commit_sha::CommitSha;

/// What to reproduce, and what the caller believes it should reproduce to.
#[derive(Debug, Clone)]
pub struct ConflictRequest {
    pub remote: String,
    pub token: Option<String>,
    pub head: BranchName,
    pub base: BranchName,
    pub expected_head: Option<CommitSha>,
    pub expected_base: Option<CommitSha>,
}
