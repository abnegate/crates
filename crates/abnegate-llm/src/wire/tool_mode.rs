use serde::{Deserialize, Serialize};

/// Whether the model may, must not, or must call a tool, without naming one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ToolMode {
    /// The model decides.
    Auto,
    /// The model must answer without a tool.
    None,
    /// The model must call at least one tool.
    Required,
}
