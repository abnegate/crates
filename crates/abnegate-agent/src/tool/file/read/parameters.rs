use serde::Deserialize;

/// The arguments a call to the tool carries.
#[derive(Debug, Deserialize)]
pub(crate) struct ReadFileParameters {
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) start_line: Option<usize>,
    #[serde(default)]
    pub(crate) end_line: Option<usize>,
    #[serde(default)]
    pub(crate) offset: Option<usize>,
    #[serde(default)]
    pub(crate) limit: Option<usize>,
}
