use crate::branch_name::BranchName;
use crate::commit_sha::CommitSha;
use crate::repository_url::RepositoryUrl;
use abnegate_secret::SecretValue;

/// What to reproduce, and what the caller believes it should reproduce to.
#[derive(Debug, Clone)]
#[non_exhaustive]
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

impl ConflictRequest {
    /// The conflict between `head` and `base` in `remote`, fetched with no
    /// credential and reproduced at whatever commits the branches are at.
    pub fn new(remote: RepositoryUrl, head: BranchName, base: BranchName) -> Self {
        Self {
            remote,
            token: None,
            head,
            base,
            expected_head: None,
            expected_base: None,
        }
    }

    /// The credential sent to the repository.
    #[must_use]
    pub fn with_token(mut self, token: SecretValue) -> Self {
        self.token = Some(token);
        self
    }

    /// The commit the caller saw the head branch at: a head that has moved
    /// away from it is refused with
    /// [`ConflictError::Moved`](crate::ConflictError::Moved).
    #[must_use]
    pub fn with_expected_head(mut self, commit: CommitSha) -> Self {
        self.expected_head = Some(commit);
        self
    }

    /// The commit the caller saw the base branch at: a base that has moved
    /// away from it is refused with
    /// [`ConflictError::Moved`](crate::ConflictError::Moved).
    #[must_use]
    pub fn with_expected_base(mut self, commit: CommitSha) -> Self {
        self.expected_base = Some(commit);
        self
    }
}
