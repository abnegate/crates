use serde::{Deserialize, Serialize};

/// The function a [`ToolChoice`](crate::ToolChoice) forces the model to call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecificFunction {
    pub name: String,
}
