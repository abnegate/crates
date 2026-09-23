use serde::Deserialize;

/// The arguments a call to the tool carries.
#[derive(Debug, Deserialize)]
pub(crate) struct WaitForParameters {
    pub(crate) id: String,
    #[serde(default, rename = "timeout_secs")]
    pub(crate) timeout_seconds: Option<u64>,
}
