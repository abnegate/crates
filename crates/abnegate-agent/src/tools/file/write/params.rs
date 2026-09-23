use serde::Deserialize;

/// The arguments a call to the tool carries.
#[derive(Debug, Deserialize)]
pub(crate) struct WriteFileParams {
    pub(crate) path: String,
    pub(crate) content: String,
    #[serde(default)]
    pub(crate) append: bool,
    #[serde(default)]
    pub(crate) reason: Option<String>,
}
