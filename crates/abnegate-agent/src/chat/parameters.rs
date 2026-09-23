use serde::Deserialize;

/// What a LiteLLM route passes to its deployment.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(super) struct Parameters {
    pub(super) model: String,
    #[serde(default)]
    pub(super) api_base: Option<String>,
    #[serde(default)]
    pub(super) num_ctx: Option<u64>,
}
