use serde::Deserialize;

/// What GitHub answers a created comment or review with, read only for the
/// identifier it was given.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubIdentified {
    pub(super) id: u64,
}
