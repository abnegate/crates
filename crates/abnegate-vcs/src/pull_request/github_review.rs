use crate::pull_request::github_user::GitHubUser;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubReview {
    #[serde(default)]
    pub(super) state: Option<String>,
    #[serde(default)]
    pub(super) body: Option<String>,
    #[serde(default)]
    pub(super) user: Option<GitHubUser>,
}
