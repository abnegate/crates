use serde::Deserialize;

/// One exact-text replacement.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PatchHunk {
    pub(crate) old_string: String,
    pub(crate) new_string: String,
}
