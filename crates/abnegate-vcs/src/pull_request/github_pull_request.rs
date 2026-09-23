use crate::pull_request::GitHubBranch;
use serde::Deserialize;
use serde::Serialize;

/// GitHub pull request response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubPullRequest {
    pub id: i64,
    pub number: i64,
    pub html_url: String,
    pub state: String,
    pub title: String,
    pub body: Option<String>,
    pub head: GitHubBranch,
    pub base: GitHubBranch,
}
