use serde::{Deserialize, Serialize};

/// The function a [`ToolChoice`](crate::ToolChoice) forces the model to call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SpecificFunction {
    pub name: String,
}

impl SpecificFunction {
    /// The function called `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}
