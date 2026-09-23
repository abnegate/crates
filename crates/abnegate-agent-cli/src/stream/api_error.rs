use serde::Deserialize;

/// The failure an `error` event reports mid-stream, such as an overloaded
/// model.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct ApiError {
    /// The API's name for the failure, such as `overloaded_error`.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}
