use serde::Deserialize;
use serde::Serialize;

/// Where a context limit came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContextSource {
    /// The window of a model already loaded in the serving runtime.
    Runtime,
    /// An operator's setting, bounded by what the model supports.
    Configured,
    /// The limit the model's provider advertises.
    Provider,
    /// No limit is known.
    Unknown,
}
