use serde::Deserialize;

/// Where a check run is in its life, as GitHub names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum GitHubCheckStatus {
    Completed,
    InProgress,
    Pending,
    Queued,
    Requested,
    Waiting,
    /// A status this crate does not know yet, which is not completed.
    #[serde(other)]
    Unknown,
}
