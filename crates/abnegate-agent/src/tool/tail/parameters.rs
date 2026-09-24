use serde::Deserialize;

/// The arguments a call to the tool carries.
#[derive(Debug, Deserialize)]
pub(crate) struct TailJobParameters {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) since: Option<u64>,
    #[serde(default, rename = "max_output_chars")]
    pub(crate) max_output_characters: Option<u64>,
}
