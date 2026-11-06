use crate::branch_name::BranchName;
use crate::commit_sha::CommitSha;

/// Which of a repository's workflow runs a list holds: every one, unless a
/// `with_*` method narrows it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkflowRunFilter {
    head_sha: Option<CommitSha>,
    event: Option<String>,
    branch: Option<BranchName>,
    created: Option<String>,
}

impl WorkflowRunFilter {
    /// Every workflow run.
    pub fn new() -> Self {
        Self::default()
    }

    /// Only the runs of `head_sha`.
    #[must_use]
    pub fn with_head_sha(mut self, head_sha: CommitSha) -> Self {
        self.head_sha = Some(head_sha);
        self
    }

    /// Only the runs `event` started, such as `push` or `release`.
    #[must_use]
    pub fn with_event(mut self, event: impl Into<String>) -> Self {
        self.event = Some(event.into());
        self
    }

    /// Only the runs of `branch`, which for a run a tag started is the tag.
    #[must_use]
    pub fn with_branch(mut self, branch: BranchName) -> Self {
        self.branch = Some(branch);
        self
    }

    /// Only the runs created when `created` says, in GitHub's search syntax
    /// for a date, such as `>=2026-01-01` or `2026-01-01..2026-01-31`.
    #[must_use]
    pub fn with_created(mut self, created: impl Into<String>) -> Self {
        self.created = Some(created.into());
        self
    }

    /// Each narrowing set, as the query parameter GitHub takes it in and its
    /// value.
    pub(super) fn pairs(&self) -> impl Iterator<Item = (&'static str, &str)> {
        [
            ("head_sha", self.head_sha.as_ref().map(CommitSha::as_str)),
            ("event", self.event.as_deref()),
            ("branch", self.branch.as_ref().map(BranchName::as_str)),
            ("created", self.created.as_deref()),
        ]
        .into_iter()
        .filter_map(|(parameter, value)| value.map(|value| (parameter, value)))
    }
}
