use crate::pull_request::CommitStatus;
use serde::Deserialize;

/// The latest status one context reported on a commit as the GitHub REST API
/// returns it, its state kept as GitHub's own word.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubCommitStatusRecord {
    context: String,
    state: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    target_url: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
}

impl From<GitHubCommitStatusRecord> for CommitStatus {
    fn from(status: GitHubCommitStatusRecord) -> Self {
        Self {
            context: status.context,
            state: status.state,
            description: status.description,
            target_url: status.target_url,
            created_at: status.created_at,
        }
    }
}
