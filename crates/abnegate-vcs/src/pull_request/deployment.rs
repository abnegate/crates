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
    /// The branch, tag or commit the deployment was asked for by, or empty
    /// when GitHub does not say.
    pub reference: String,
    /// Where it was deployed to, such as `production`, or empty when GitHub
    /// does not say.
    pub environment: String,
    /// When it was created, as RFC 3339, or empty when GitHub does not say.
    pub created_at: String,
    /// When it last changed, as RFC 3339, or empty when GitHub does not say.
    pub updated_at: String,
    /// The REST address GitHub answers for the deployment at.
    pub url: String,
    /// The latest status its provider gave, or `None` before it gave any.
    pub status: Option<DeploymentStatus>,
}

impl Deployment {
    /// The deployment GitHub identifies by `id`, of `sha`, answered for at
    /// `url`, with nothing else said of it.
    pub fn new(id: u64, sha: CommitSha, url: impl Into<String>) -> Self {
        Self {
            id,
            sha,
            reference: String::new(),
            environment: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            url: url.into(),
            status: None,
        }
    }

    /// The branch, tag or commit the deployment was asked for by.
    #[must_use]
    pub fn with_reference(mut self, reference: impl Into<String>) -> Self {
        self.reference = reference.into();
        self
    }

    /// Where it was deployed to.
    #[must_use]
    pub fn with_environment(mut self, environment: impl Into<String>) -> Self {
        self.environment = environment.into();
        self
    }

    /// When it was created, as RFC 3339.
    #[must_use]
    pub fn with_created_at(mut self, created_at: impl Into<String>) -> Self {
        self.created_at = created_at.into();
        self
    }

    /// When it last changed, as RFC 3339.
    #[must_use]
    pub fn with_updated_at(mut self, updated_at: impl Into<String>) -> Self {
        self.updated_at = updated_at.into();
        self
    }

    /// The latest status its provider gave.
    #[must_use]
    pub fn with_status(mut self, status: DeploymentStatus) -> Self {
        self.status = Some(status);
        self
    }
}
