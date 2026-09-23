use serde::Deserialize;

use crate::catalog::openrouter::model::OpenRouterModel;

#[derive(Debug, Deserialize)]
pub(crate) struct OpenRouterResponse {
    pub(crate) data: Vec<OpenRouterModel>,
}
