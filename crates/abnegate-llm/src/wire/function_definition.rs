use serde::{Deserialize, Serialize};

/// The function a [`ToolDefinition`](crate::ToolDefinition) offers the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}
