use crate::pull_request::github_signature::GitHubSignature;
use serde::Deserialize;

/// The git commit itself inside a commit GitHub's REST API lists: its message
/// and who wrote it.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubCommitDetail {
    pub(super) message: String,
    #[serde(default)]
    pub(super) author: Option<GitHubSignature>,
}
