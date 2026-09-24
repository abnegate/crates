use crate::pull_request::IssueComment;
use crate::pull_request::github_user::GitHubUser;
use serde::Deserialize;

/// A comment on a pull request's conversation as the GitHub REST API returns
/// it.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubIssueComment {
    id: u64,
    #[serde(default)]
    user: Option<GitHubUser>,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    created_at: String,
}

impl From<GitHubIssueComment> for IssueComment {
    fn from(comment: GitHubIssueComment) -> Self {
        Self {
            id: comment.id,
            author: comment.user.and_then(|user| user.login).unwrap_or_default(),
            body: comment.body.unwrap_or_default(),
            url: comment.html_url,
            created_at: comment.created_at,
        }
    }
}
