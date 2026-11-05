use crate::commit_sha::CommitSha;
use crate::pull_request::Deployment;
use crate::pull_request::PullRequestResult;
use serde::Deserialize;

/// A deployment as the GitHub REST API lists it.
///
/// Everything read of it is required, as GitHub sends it on every deployment,
/// so an answer that leaves any of it out is unreadable rather than a
/// deployment with something made up in its place.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubDeployment {
    pub(super) id: u64,
    sha: String,
    #[serde(rename = "ref")]
    reference: String,
    environment: String,
    created_at: String,
    updated_at: String,
}

impl GitHubDeployment {
    /// The deployment this row describes, answered for by the REST API at
    /// `api_url`, before any status is read; a commit this crate does not
    /// accept is [`crate::pull_request::PullRequestError::Parse`].
    pub(super) fn answered_at(self, api_url: String) -> PullRequestResult<Deployment> {
        Ok(Deployment {
            id: self.id,
            sha: CommitSha::parse(&self.sha)?,
            reference: self.reference,
            environment: self.environment,
            created_at: self.created_at,
            updated_at: self.updated_at,
            api_url,
            status: None,
        })
    }
}
