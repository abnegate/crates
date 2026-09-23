use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct OpenRouterArchitecture {
    #[serde(default)]
    pub(crate) tokenizer: Option<String>,
    #[serde(default)]
    pub(crate) modality: Option<String>,
    #[serde(default)]
    pub(crate) input_modalities: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) output_modalities: Option<Vec<String>>,
}
