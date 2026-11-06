use serde::Deserialize;

/// Where codex's call to one of its collaboration tools stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CollabStatus {
    InProgress,
    Completed,
    Failed,
}
