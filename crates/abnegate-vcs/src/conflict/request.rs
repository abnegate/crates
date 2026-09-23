use crate::branch_name::BranchName;
use crate::commit_sha::CommitSha;
use crate::repository_url::RepositoryUrl;
use abnegate_secret::SecretValue;

/// What to reproduce, and what the caller believes it should reproduce to.
#[derive(Debug, Clone)]
pub struct ConflictRequest {
    /// The repository both branches are fetched from and the repair is published to.
    pub remote: RepositoryUrl,
    /// The credential sent to that repository, if it needs one.
    pub token: Option<SecretValue>,
    /// The branch that conflicts, which a repair is published to.
    pub head: BranchName,
    /// The branch it conflicts with.
    pub base: BranchName,
    /// The commit the caller saw the head branch at, if it saw one.
    pub expected_head: Option<CommitSha>,
    /// The commit the caller saw the base branch at, if it saw one.
    pub expected_base: Option<CommitSha>,
}
