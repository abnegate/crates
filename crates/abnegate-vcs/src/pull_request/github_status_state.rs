use serde::Deserialize;

/// What one commit status reports, as GitHub names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum GitHubStatusState {
    Error,
    Failure,
    Pending,
    Success,
    /// A state this crate does not know yet, which fails nothing but is no
    /// pass either, so it keeps the commit pending.
    #[serde(other)]
    Unknown,
}
