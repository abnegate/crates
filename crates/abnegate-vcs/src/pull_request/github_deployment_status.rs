use crate::pull_request::DeploymentStatus;
use serde::Deserialize;

/// A deployment's status as the GitHub REST API lists it.
///
/// Its state, description and time are required, as GitHub sends them on
/// every status, so an answer that leaves any of them out is unreadable. Where
/// the deployment can be reached and its output read are the provider's to
/// give, and are read as not said when GitHub leaves them out or sends `null`.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubDeploymentStatus {
    state: String,
    description: String,
    #[serde(default)]
    environment_url: Option<String>,
    #[serde(default)]
    log_url: Option<String>,
    created_at: String,
}

impl From<GitHubDeploymentStatus> for DeploymentStatus {
    fn from(status: GitHubDeploymentStatus) -> Self {
        Self {
            state: status.state,
            description: status.description,
            environment_url: status.environment_url,
            log_url: status.log_url,
            created_at: status.created_at,
        }
    }
}
