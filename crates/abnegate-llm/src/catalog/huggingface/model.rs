use serde::Deserialize;

use crate::catalog::huggingface::card_data::HuggingFaceCardData;
use crate::catalog::huggingface::gguf::HuggingFaceGguf;
use crate::catalog::huggingface::sibling::HuggingFaceSibling;

#[derive(Debug, Deserialize)]
pub(crate) struct HuggingFaceModel {
    #[serde(default)]
    pub(crate) id: Option<String>,
    #[serde(rename = "modelId", default)]
    pub(crate) model_id: Option<String>,
    #[serde(default)]
    pub(crate) sha: Option<String>,
    #[serde(rename = "lastModified", default)]
    pub(crate) last_modified: Option<String>,
    #[serde(rename = "createdAt", default)]
    pub(crate) created_at: Option<String>,
    #[serde(default)]
    pub(crate) tags: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) downloads: Option<u64>,
    #[serde(default)]
    pub(crate) likes: Option<u64>,
    #[serde(default)]
    pub(crate) author: Option<String>,
    #[serde(rename = "pipeline_tag", default)]
    pub(crate) pipeline_tag: Option<String>,
    #[serde(rename = "cardData", default)]
    pub(crate) card_data: Option<HuggingFaceCardData>,
    #[serde(default)]
    pub(crate) gguf: Option<HuggingFaceGguf>,
    #[serde(default)]
    pub(crate) siblings: Option<Vec<HuggingFaceSibling>>,
}
