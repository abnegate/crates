use crate::pull_request::github_tree_entry::GitHubTreeEntry;
use serde::Deserialize;

/// A git tree as GitHub's REST API returns it. Whether GitHub cut the list
/// short is required, so a list that does not say is never taken for a whole
/// one.
#[derive(Debug, Deserialize)]
pub(super) struct GitHubTree {
    pub(super) sha: String,
    pub(super) tree: Vec<GitHubTreeEntry>,
    pub(super) truncated: bool,
}
