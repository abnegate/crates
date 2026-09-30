use serde::Deserialize;
use serde::Serialize;

/// Format, family and runtime facts about a catalogued model.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct ModelDetails {
    /// The weight file format, such as `gguf`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// The architecture family, such as `llama`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// The parameter count as the catalogue writes it, such as `8B`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameter_size: Option<String>,
    /// The quantization, such as `Q4_0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantization_level: Option<String>,
    /// The context window, in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
    /// The licence the model is published under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// The memory the model needs to run, in gigabytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ram_required_gb: Option<u64>,
}
