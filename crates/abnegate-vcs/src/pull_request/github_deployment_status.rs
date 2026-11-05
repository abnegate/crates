use crate::pull_request::DeploymentStatus;
use serde::Deserialize;

/// A deployment's status as the GitHub REST API lists it.
///
/// Nothing is required: GitHub sends a state and a time on every status, but
/// an answer that leaves them out is still the latest one.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubDeploymentStatus {
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    environment_url: Option<String>,
    #[serde(default)]
    log_url: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
}

impl From<GitHubDeploymentStatus> for DeploymentStatus {
    fn from(status: GitHubDeploymentStatus) -> Self {
        Self {
            state: status.state.unwrap_or_default(),
            description: status.description,
            environment_url: status.environment_url,
            log_url: status.log_url,
            created_at: status.created_at.unwrap_or_default(),
        }
    }
}
