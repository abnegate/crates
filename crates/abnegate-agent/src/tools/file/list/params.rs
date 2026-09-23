use serde::Deserialize;

/// The arguments a call to the tool carries.
#[derive(Debug, Deserialize)]
pub(crate) struct ListFilesParams {
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) recursive: bool,
    #[serde(default)]
    pub(crate) pattern: Option<String>,
}
