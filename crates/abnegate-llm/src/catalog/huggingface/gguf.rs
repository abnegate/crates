use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct HuggingFaceGguf {
    #[serde(default)]
    pub(crate) total: Option<u64>,
    #[serde(rename = "totalFileSize", default)]
    pub(crate) total_file_size: Option<u64>,
    #[serde(default)]
    pub(crate) architecture: Option<String>,
    #[serde(rename = "context_length", default)]
    pub(crate) context_length: Option<u64>,
}
