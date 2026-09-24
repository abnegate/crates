use serde::Deserialize;

/// The arguments a call to the tool carries.
#[derive(Debug, Deserialize)]
pub(crate) struct RunShellParameters {
    pub(crate) command: String,
    #[serde(default, rename = "cwd")]
    pub(crate) working_directory: Option<String>,
    #[serde(default, rename = "timeout_secs")]
    pub(crate) timeout_seconds: Option<u64>,
    #[serde(default)]
    pub(crate) reason: Option<String>,
    #[serde(default, rename = "max_output_chars")]
    pub(crate) maximum_output_characters: Option<u64>,
    #[serde(default)]
    pub(crate) background: bool,
}
