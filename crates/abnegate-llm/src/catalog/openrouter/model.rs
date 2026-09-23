use serde::Deserialize;

use crate::catalog::openrouter::architecture::OpenRouterArchitecture;

#[derive(Debug, Deserialize)]
pub(crate) struct OpenRouterModel {
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) context_length: Option<u64>,
    #[serde(default)]
    pub(crate) architecture: Option<OpenRouterArchitecture>,
    #[serde(default)]
    pub(crate) supported_parameters: Option<Vec<String>>,
}

impl OpenRouterModel {
    pub(crate) fn matches(&self, needle: &str) -> bool {
        self.id.to_lowercase().contains(needle)
            || self.name.to_lowercase().contains(needle)
            || self
                .description
                .as_ref()
                .is_some_and(|value| value.to_lowercase().contains(needle))
    }
}
