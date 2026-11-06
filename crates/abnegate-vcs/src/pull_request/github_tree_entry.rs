use crate::pull_request::TreeEntryKind;
use serde::Deserialize;

/// One entry of a git tree as GitHub's REST API lists it. A blob carries its
/// size; any other entry does not.
#[derive(Debug, Deserialize)]
pub(super) struct GitHubTreeEntry {
    pub(super) path: String,
    pub(super) mode: String,
    #[serde(rename = "type")]
    pub(super) kind: TreeEntryKind,
    pub(super) sha: String,
    #[serde(default)]
    pub(super) size: Option<u64>,
}
