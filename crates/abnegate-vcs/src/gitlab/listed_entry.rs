use crate::tree_entry_kind::TreeEntryKind;
use serde::Deserialize;

/// One entry of a repository tree as GitLab's REST API lists it, with no
/// size.
#[derive(Debug, Deserialize)]
pub(super) struct ListedEntry {
    pub(super) id: String,
    pub(super) path: String,
    #[serde(rename = "type")]
    pub(super) kind: TreeEntryKind,
    pub(super) mode: String,
}
