use serde::{Deserialize, Serialize};

/// The function a [`ToolDefinition`](crate::ToolDefinition) offers the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl FunctionDefinition {
    /// A function called `name`, described to the model as `description`,
    /// taking arguments that match the JSON schema `parameters`.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}
