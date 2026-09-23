use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct HuggingFaceSibling {
    pub(crate) rfilename: String,
    #[serde(default)]
    pub(crate) size: Option<u64>,
}
