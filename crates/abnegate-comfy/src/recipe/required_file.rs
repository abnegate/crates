use serde::Deserialize;

/// A model file a recipe's graph loads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct RequiredFile {
    /// The file's name.
    pub filename: String,
    /// The folder under the models directory it belongs in.
    pub directory: String,
}
