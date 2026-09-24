use crate::pull_request::github_user::GitHubUser;
use serde::Deserialize;

/// One comment in a review thread, as GitHub's GraphQL API answers for it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GitHubThreadComment {
    #[serde(default)]
    pub(super) database_id: Option<u64>,
    pub(super) body: String,
    pub(super) url: String,
    pub(super) created_at: String,
    #[serde(default)]
    pub(super) author: Option<GitHubUser>,
}
