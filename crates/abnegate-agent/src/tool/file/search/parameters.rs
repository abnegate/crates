use serde::Deserialize;

/// The arguments a call to the tool carries.
#[derive(Debug, Deserialize)]
pub(crate) struct SearchCodeParameters {
    pub(crate) pattern: String,
    pub(crate) path: Option<String>,
    #[serde(default)]
    pub(crate) case_sensitive: bool,
    #[serde(default, rename = "max_results")]
    pub(crate) maximum_results: Option<usize>,
}
