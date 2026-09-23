use crate::branch_name::BranchName;
use crate::commit_sha::CommitSha;
use crate::repository_url::RepositoryUrl;
use abnegate_secret::SecretValue;

/// What to reproduce, and what the caller believes it should reproduce to.
#[derive(Debug, Clone)]
pub struct ConflictRequest {
    pub remote: RepositoryUrl,
    pub token: Option<SecretValue>,
    pub head: BranchName,
    pub base: BranchName,
    pub expected_head: Option<CommitSha>,
    pub expected_base: Option<CommitSha>,
}
