use serde::Deserialize;

/// The part of GitHub's answer to a completed merge this service reads.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubMerge {
    pub(super) sha: Option<String>,
}
