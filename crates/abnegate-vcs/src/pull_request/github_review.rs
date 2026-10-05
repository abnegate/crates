use crate::pull_request::github_user::GitHubUser;
use serde::Deserialize;

/// A submitted review as the GitHub REST API returns it.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubReview {
    #[serde(default)]
    pub(super) id: Option<u64>,
    #[serde(default)]
    pub(super) state: Option<String>,
    #[serde(default)]
    pub(super) body: Option<String>,
    #[serde(default)]
    pub(super) user: Option<GitHubUser>,
    #[serde(default)]
    pub(super) commit_id: Option<String>,
    #[serde(default)]
    pub(super) submitted_at: Option<String>,
}
