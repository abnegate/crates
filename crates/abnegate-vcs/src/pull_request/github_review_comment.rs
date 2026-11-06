use crate::pull_request::ReviewComment;
use crate::pull_request::github_user::GitHubUser;
use serde::Deserialize;

/// A comment on a pull request's diff as the GitHub REST API returns it.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubReviewComment {
    pub(super) id: u64,
    #[serde(default)]
    pub(super) user: Option<GitHubUser>,
    #[serde(default)]
    pub(super) body: Option<String>,
    #[serde(default)]
    pub(super) path: Option<String>,
    pub(super) html_url: String,
    pub(super) created_at: String,
    #[serde(default)]
    pub(super) pull_request_url: Option<String>,
}

impl From<GitHubReviewComment> for ReviewComment {
    fn from(comment: GitHubReviewComment) -> Self {
        Self {
            id: comment.id,
            author: comment.user.and_then(|user| user.login).unwrap_or_default(),
            body: comment.body.unwrap_or_default(),
            path: comment.path,
            url: comment.html_url,
            created_at: comment.created_at,
        }
    }
}
