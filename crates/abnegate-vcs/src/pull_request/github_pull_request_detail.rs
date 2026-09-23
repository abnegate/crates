use crate::pull_request::PullRequestState;
use serde::Deserialize;

/// The part of a pull request's details its reception and mergeability are
/// read from.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubPullRequestDetail {
    #[serde(default)]
    pub(super) created_at: Option<String>,
    #[serde(default)]
    pub(super) merged_at: Option<String>,
    #[serde(default)]
    pub(super) state: Option<PullRequestState>,
    #[serde(default)]
    pub(super) mergeable: Option<bool>,
}
