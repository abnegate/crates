use crate::commit_sha::CommitSha;
use crate::pull_request::DeploymentStatus;

/// A deployment of a commit to an environment, with the latest status its
/// provider gave.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Deployment {
    /// GitHub's identifier for the deployment.
    pub id: u64,
    /// The commit GitHub says was deployed.
    pub sha: CommitSha,
    /// The branch, tag or commit the deployment was asked for by.
    pub reference: String,
    /// Where it was deployed to, such as `production`.
    pub environment: String,
    /// When it was created, as RFC 3339.
    pub created_at: String,
    /// When it last changed, as RFC 3339.
    pub updated_at: String,
    /// The address GitHub's REST API answers for the deployment at, not one a
    /// browser reads it at.
    pub api_url: String,
    /// The latest status its provider gave, or `None` before it gave any.
    pub status: Option<DeploymentStatus>,
}

impl Deployment {
    /// The deployment GitHub identifies by `id`, of `sha`, asked for by
    /// `reference`, to `environment`, created at `created_at` and last
    /// changed at `updated_at`, both as RFC 3339, and answered for by the
    /// REST API at `api_url`, before its provider gave any status.
    pub fn new(
        id: u64,
        sha: CommitSha,
        reference: impl Into<String>,
        environment: impl Into<String>,
        created_at: impl Into<String>,
        updated_at: impl Into<String>,
        api_url: impl Into<String>,
    ) -> Self {
        Self {
            id,
            sha,
            reference: reference.into(),
            environment: environment.into(),
            created_at: created_at.into(),
            updated_at: updated_at.into(),
            api_url: api_url.into(),
            status: None,
        }
    }

    /// The latest status its provider gave.
    #[must_use]
    pub fn with_status(mut self, status: DeploymentStatus) -> Self {
        self.status = Some(status);
        self
    }
}
