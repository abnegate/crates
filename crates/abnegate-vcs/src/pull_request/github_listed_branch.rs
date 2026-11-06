use serde::Deserialize;

/// One side of a pull request in a list GitHub's REST API returns: the branch,
/// the `owner:branch` label that says whose it is, and the commit at its tip.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubListedBranch {
    #[serde(rename = "ref")]
    pub(super) reference: String,
    #[serde(default)]
    pub(super) label: Option<String>,
    #[serde(default)]
    pub(super) sha: Option<String>,
}
