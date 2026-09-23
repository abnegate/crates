use serde::Deserialize;

/// The arguments a call to the tool carries.
#[derive(Debug, Deserialize)]
pub(crate) struct SearchCodeParams {
    pub(crate) pattern: String,
    pub(crate) path: Option<String>,
    #[serde(default)]
    pub(crate) case_sensitive: bool,
    #[serde(default)]
    pub(crate) max_results: Option<usize>,
}
