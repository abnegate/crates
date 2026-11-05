use crate::commit_sha::CommitSha;
use crate::pull_request::Deployment;
use crate::pull_request::PullRequestResult;
use serde::Deserialize;

/// A deployment as the GitHub REST API lists it.
///
/// Only its identifier and commit are required: GitHub sends the rest on
/// every deployment, but an answer that leaves any of it out still names the
/// deployment.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubDeployment {
    pub(super) id: u64,
    sha: String,
    #[serde(default, rename = "ref")]
    reference: Option<String>,
    #[serde(default)]
    environment: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

impl GitHubDeployment {
    /// The deployment this row describes, answered for at `url`, before any
    /// status is read; a commit this crate does not accept is
    /// [`crate::pull_request::PullRequestError::Parse`].
    pub(super) fn answered_at(self, url: String) -> PullRequestResult<Deployment> {
        Ok(Deployment {
            id: self.id,
            sha: CommitSha::parse(&self.sha)?,
            reference: self.reference.unwrap_or_default(),
            environment: self.environment.unwrap_or_default(),
            created_at: self.created_at.unwrap_or_default(),
            updated_at: self.updated_at.unwrap_or_default(),
            url,
            status: None,
        })
    }
}
