use serde::Deserialize;

/// A function call arriving one fragment at a time.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct StreamFunctionCall {
    /// The function's name, usually only on the call's first fragment.
    pub name: Option<String>,
    /// More of the arguments' JSON text, which parses only once joined.
    pub arguments: Option<String>,
}
