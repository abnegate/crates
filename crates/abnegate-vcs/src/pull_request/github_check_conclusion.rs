use serde::Deserialize;

/// How a finished check run concluded, as GitHub names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum GitHubCheckConclusion {
    ActionRequired,
    Cancelled,
    Error,
    Failure,
    Neutral,
    Skipped,
    Stale,
    StartupFailure,
    Success,
    TimedOut,
    /// A conclusion this crate does not know yet, which fails nothing.
    #[serde(other)]
    Unknown,
}
