use serde::Deserialize;

/// A function call arriving one fragment at a time.
#[derive(Debug, Clone, Deserialize)]
pub struct StreamFunctionCall {
    pub name: Option<String>,
    pub arguments: Option<String>,
}
