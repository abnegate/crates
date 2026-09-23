use crate::inventory::WeightSidecar;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct WeightDocument {
    #[serde(flatten)]
    pub sidecar: WeightSidecar,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
}
