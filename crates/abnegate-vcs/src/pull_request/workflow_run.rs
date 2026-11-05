use crate::commit_sha::CommitSha;
use std::num::NonZeroU64;

/// One run of a GitHub Actions workflow, in GitHub's own words.
///
/// Its status, conclusion and event are GitHub's words as it sent them, so a
/// word GitHub adds later reaches the caller rather than an error. A field
/// GitHub may leave out or send as `null` is `None`; every other field is one
/// GitHub always sends, and a run without it is never read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkflowRun {
    /// GitHub's identifier for the run.
    pub id: NonZeroU64,
    /// GitHub's identifier for the workflow the run is of.
    pub workflow_id: NonZeroU64,
    /// Which attempt at the run this is, counting from 1.
    pub attempt: Option<u32>,
    /// The workflow's name.
    pub name: Option<String>,
    /// Where the workflow is defined in the repository, such as
    /// `.github/workflows/ci.yml`.
    pub path: String,
    /// What started the run, such as `push`, `pull_request` or `release`.
    pub event: String,
    /// Where the run is, such as `queued`, `in_progress` or `completed`.
    pub status: Option<String>,
    /// How the run finished, such as `success` or `failure`, once it has.
    pub conclusion: Option<String>,
    /// The branch the run is of.
    pub head_branch: Option<String>,
    /// The commit the run is of.
    pub head_sha: CommitSha,
    /// When the run was created, as RFC 3339.
    pub created_at: String,
    /// When the run's current attempt started, as RFC 3339.
    pub started_at: Option<String>,
    /// When the run last changed, as RFC 3339.
    pub updated_at: String,
    /// Where the run can be read in a browser: GitHub's `html_url`.
    pub url: String,
    /// The `owner/name` of the repository the run is in: GitHub's
    /// `repository.full_name`.
    pub repository: String,
}

impl WorkflowRun {
    /// The run GitHub identifies by `id`, of the workflow `workflow_id`
    /// defined at `path`, which `event` started on `head_sha`, created at
    /// `created_at` and last changed at `updated_at`, readable at `url` and in
    /// the repository named `repository`.
    ///
    /// Its attempt, name, status, conclusion, branch and start are unknown
    /// until the `with_*` methods say otherwise.
    pub fn new(
        id: NonZeroU64,
        workflow_id: NonZeroU64,
        path: impl Into<String>,
        event: impl Into<String>,
        head_sha: CommitSha,
        created_at: impl Into<String>,
        updated_at: impl Into<String>,
        url: impl Into<String>,
        repository: impl Into<String>,
    ) -> Self {
        Self {
            id,
            workflow_id,
            attempt: None,
            name: None,
            path: path.into(),
            event: event.into(),
            status: None,
            conclusion: None,
            head_branch: None,
            head_sha,
            created_at: created_at.into(),
            started_at: None,
            updated_at: updated_at.into(),
            url: url.into(),
            repository: repository.into(),
        }
    }

    /// Which attempt at the run this is, counting from 1.
    #[must_use]
    pub fn with_attempt(mut self, attempt: u32) -> Self {
        self.attempt = Some(attempt);
        self
    }

    /// The workflow's name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Where the run is.
    #[must_use]
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    /// How the run finished.
    #[must_use]
    pub fn with_conclusion(mut self, conclusion: impl Into<String>) -> Self {
        self.conclusion = Some(conclusion.into());
        self
    }

    /// The branch the run is of.
    #[must_use]
    pub fn with_head_branch(mut self, head_branch: impl Into<String>) -> Self {
        self.head_branch = Some(head_branch.into());
        self
    }

    /// When the run's current attempt started, as RFC 3339.
    #[must_use]
    pub fn with_started_at(mut self, started_at: impl Into<String>) -> Self {
        self.started_at = Some(started_at.into());
        self
    }
}
