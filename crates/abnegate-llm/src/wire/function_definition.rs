use serde::Deserialize;
use serde::Serialize;

/// The function a [`ToolDefinition`](crate::ToolDefinition) offers the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FunctionDefinition {
    /// The name the model calls the function by.
    pub name: String,
    /// What the function does, as the model reads it.
    pub description: String,
    /// JSON Schema for its arguments.
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
