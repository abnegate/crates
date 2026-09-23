use serde::Deserialize;
use serde_json::Value;

use super::parameters::Parameters;

/// One LiteLLM deployment route.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct Route {
    pub(super) model_name: String,
    pub(super) litellm_params: Parameters,
    #[serde(default)]
    pub(super) model_info: Value,
}
