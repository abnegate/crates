use serde::Deserialize;

/// What GitHub answers a created git object with, read only for its name.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubObject {
    pub(super) sha: String,
}
