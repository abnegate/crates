use serde::Deserialize;
use serde::Serialize;

/// One side of a pull request as the GitHub REST API returns it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubBranch {
    #[serde(rename = "ref")]
    pub reference: String,
    pub sha: String,
}
