use crate::commit_sha::CommitSha;
use std::num::NonZeroU64;

/// One job of a GitHub Actions workflow run, in GitHub's own words.
///
/// Its status and conclusion are GitHub's words as it sent them. A field
/// GitHub may leave out or send as `null` is `None`; every other field is one
/// GitHub always sends, and a job without it is never read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkflowJob {
    /// GitHub's identifier for the job.
    pub id: NonZeroU64,
    /// GitHub's identifier for the run the job is part of.
    pub run_id: NonZeroU64,
    /// The job's name.
    pub name: String,
    /// Where the job is, such as `queued`, `in_progress` or `completed`.
    pub status: String,
    /// How the job finished, such as `success` or `failure`, once it has.
    pub conclusion: Option<String>,
    /// The commit the job ran on.
    pub head_sha: CommitSha,
    /// Where the job can be read in a browser: GitHub's `html_url`.
    pub url: Option<String>,
    /// When the job started, as RFC 3339.
    pub started_at: Option<String>,
    /// When the job finished, as RFC 3339.
    pub completed_at: Option<String>,
}

impl WorkflowJob {
    /// The job GitHub identifies by `id`, named `name`, of the run `run_id`
    /// on `head_sha`, at `status`.
    ///
    /// Its conclusion, address, start and finish are unknown until the
    /// `with_*` methods say otherwise.
    pub fn new(
        id: NonZeroU64,
        run_id: NonZeroU64,
        name: impl Into<String>,
        status: impl Into<String>,
        head_sha: CommitSha,
    ) -> Self {
        Self {
            id,
            run_id,
            name: name.into(),
            status: status.into(),
            conclusion: None,
            head_sha,
            url: None,
            started_at: None,
            completed_at: None,
        }
    }

    /// How the job finished.
    #[must_use]
    pub fn with_conclusion(mut self, conclusion: impl Into<String>) -> Self {
        self.conclusion = Some(conclusion.into());
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
