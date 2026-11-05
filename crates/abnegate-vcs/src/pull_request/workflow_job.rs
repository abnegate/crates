use crate::commit_sha::CommitSha;

/// One job of a GitHub Actions workflow run, in GitHub's own words.
///
/// Its status and conclusion are GitHub's words as it sent them. A field
/// GitHub may leave out or send as `null` is `None`; a text GitHub always
/// sends is empty when it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkflowJob {
    /// GitHub's identifier for the job.
    pub id: u64,
    /// GitHub's identifier for the run the job is part of.
    pub run_id: Option<u64>,
    /// The job's name.
    pub name: String,
    /// Where the job is, such as `queued`, `in_progress` or `completed`.
    pub status: String,
    /// How the job finished, such as `success` or `failure`, once it has.
    pub conclusion: Option<String>,
    /// The commit the job ran on.
    pub head_sha: Option<CommitSha>,
    /// Where the job can be read in a browser.
    pub url: Option<String>,
    /// When the job started, as RFC 3339.
    pub started_at: Option<String>,
    /// When the job finished, as RFC 3339.
    pub completed_at: Option<String>,
}

impl WorkflowJob {
    /// The job GitHub identifies by `id`, with every other field empty or
    /// `None` until the `with_*` methods say otherwise.
    pub fn new(id: u64) -> Self {
        Self {
            id,
            run_id: None,
            name: String::new(),
            status: String::new(),
            conclusion: None,
            head_sha: None,
            url: None,
            started_at: None,
            completed_at: None,
        }
    }

    /// GitHub's identifier for the run the job is part of.
    #[must_use]
    pub fn with_run_id(mut self, run_id: u64) -> Self {
        self.run_id = Some(run_id);
        self
    }

    /// The job's name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Where the job is.
    #[must_use]
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = status.into();
        self
    }

    /// How the job finished.
    #[must_use]
    pub fn with_conclusion(mut self, conclusion: impl Into<String>) -> Self {
        self.conclusion = Some(conclusion.into());
        self
    }

    /// The commit the job ran on.
    #[must_use]
    pub fn with_head_sha(mut self, head_sha: CommitSha) -> Self {
        self.head_sha = Some(head_sha);
        self
    }

    /// Where the job can be read in a browser.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// When the job started, as RFC 3339.
    #[must_use]
    pub fn with_started_at(mut self, started_at: impl Into<String>) -> Self {
        self.started_at = Some(started_at.into());
        self
    }

    /// When the job finished, as RFC 3339.
    #[must_use]
    pub fn with_completed_at(mut self, completed_at: impl Into<String>) -> Self {
        self.completed_at = Some(completed_at.into());
        self
    }
}
