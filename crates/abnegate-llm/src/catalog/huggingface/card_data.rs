use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct HuggingFaceCardData {
    #[serde(default)]
    pub(crate) license: Option<String>,
    #[serde(rename = "pipeline_tag", default)]
    pub(crate) pipeline_tag: Option<String>,
    #[serde(default)]
    pub(crate) base_model: Option<serde_json::Value>,
}
