use serde::Deserialize;
use serde::Serialize;

/// One side of a pull request as the GitHub REST API returns it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubBranch {
    /// The branch name.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The commit at its tip.
    pub sha: String,
}
