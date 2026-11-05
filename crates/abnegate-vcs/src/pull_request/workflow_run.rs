use crate::commit_sha::CommitSha;

/// One run of a GitHub Actions workflow, in GitHub's own words.
///
/// Its status, conclusion and event are GitHub's words as it sent them, so a
/// word GitHub adds later reaches the caller rather than an error. A field
/// GitHub may leave out or send as `null` is `None`; a text GitHub always
/// sends is empty when it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkflowRun {
    /// GitHub's identifier for the run, or 0 when GitHub does not say.
    pub id: u64,
    /// GitHub's identifier for the workflow the run is of.
    pub workflow_id: Option<u64>,
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
    pub head_sha: Option<CommitSha>,
    /// When the run was created, as RFC 3339.
    pub created_at: String,
    /// When the run's current attempt started, as RFC 3339.
    pub started_at: Option<String>,
    /// When the run last changed, as RFC 3339.
    pub updated_at: String,
    /// Where the run can be read in a browser.
    pub url: String,
    /// The `owner/name` of the repository the run is in.
    pub repository: String,
}

impl WorkflowRun {
    /// The run GitHub identifies by `id`, with every other field empty or
    /// `None` until the `with_*` methods say otherwise.
    pub fn new(id: u64) -> Self {
        Self {
            id,
            workflow_id: None,
            attempt: None,
            name: None,
            path: String::new(),
            event: String::new(),
            status: None,
            conclusion: None,
            head_branch: None,
            head_sha: None,
            created_at: String::new(),
            started_at: None,
            updated_at: String::new(),
            url: String::new(),
            repository: String::new(),
        }
    }

    /// GitHub's identifier for the workflow the run is of.
    #[must_use]
    pub fn with_workflow_id(mut self, workflow_id: u64) -> Self {
        self.workflow_id = Some(workflow_id);
        self
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

    /// Where the workflow is defined in the repository.
    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    /// What started the run.
    #[must_use]
    pub fn with_event(mut self, event: impl Into<String>) -> Self {
        self.event = event.into();
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

    /// The commit the run is of.
    #[must_use]
    pub fn with_head_sha(mut self, head_sha: CommitSha) -> Self {
        self.head_sha = Some(head_sha);
        self
    }

    /// When the run was created, as RFC 3339.
    #[must_use]
    pub fn with_created_at(mut self, created_at: impl Into<String>) -> Self {
        self.created_at = created_at.into();
        self
    }

    /// When the run's current attempt started, as RFC 3339.
    #[must_use]
    pub fn with_started_at(mut self, started_at: impl Into<String>) -> Self {
        self.started_at = Some(started_at.into());
        self
    }

    /// When the run last changed, as RFC 3339.
    #[must_use]
    pub fn with_updated_at(mut self, updated_at: impl Into<String>) -> Self {
        self.updated_at = updated_at.into();
        self
    }

    /// Where the run can be read in a browser.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// The `owner/name` of the repository the run is in.
    #[must_use]
    pub fn with_repository(mut self, repository: impl Into<String>) -> Self {
        self.repository = repository.into();
        self
    }
}
