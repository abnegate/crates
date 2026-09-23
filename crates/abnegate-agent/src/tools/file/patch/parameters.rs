use serde::Deserialize;

use super::hunk::PatchHunk;
use crate::tools::ToolError;

/// The arguments a call to the tool carries.
#[derive(Debug, Deserialize)]
pub(crate) struct ApplyPatchParameters {
    pub(crate) path: String,
    pub(crate) old_string: Option<String>,
    pub(crate) new_string: Option<String>,
    #[serde(default)]
    pub(crate) hunks: Vec<PatchHunk>,
    #[serde(default)]
    pub(crate) replace_all: bool,
    #[serde(default)]
    pub(crate) reason: Option<String>,
}

impl ApplyPatchParameters {
    pub(crate) fn hunks(&self) -> Result<Vec<PatchHunk>, ToolError> {
        let mut hunks = self.hunks.clone();
        match (&self.old_string, &self.new_string) {
            (Some(old_string), Some(new_string)) => hunks.insert(
                0,
                PatchHunk {
                    old_string: old_string.clone(),
                    new_string: new_string.clone(),
                },
            ),
            (None, None) => {}
            _ => {
                return Err(ToolError::InvalidParameters(
                    "old_string and new_string must be supplied together".to_string(),
                ));
            }
        }
        if hunks.is_empty() {
            return Err(ToolError::InvalidParameters(
                "Provide old_string and new_string, or a non-empty hunks array".to_string(),
            ));
        }
        for hunk in &hunks {
            if hunk.old_string.is_empty() {
                return Err(ToolError::InvalidParameters(
                    "old_string must not be empty".to_string(),
                ));
            }
            if hunk.old_string == hunk.new_string {
                return Err(ToolError::InvalidParameters(
                    "old_string and new_string are identical".to_string(),
                ));
            }
        }
        Ok(hunks)
    }
}
