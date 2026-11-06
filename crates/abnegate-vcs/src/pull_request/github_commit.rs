use serde::Deserialize;

/// A commit as the GitHub REST API returns it, read only for its identifier.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubCommit {
    pub(super) sha: String,
}
