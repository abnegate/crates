use serde::Deserialize;
use serde::Serialize;

/// Whether a pull request is still open. A merged one is closed and carries a
/// merge time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum PullRequestState {
    Open,
    Closed,
    #[serde(other)]
    Unknown,
}
