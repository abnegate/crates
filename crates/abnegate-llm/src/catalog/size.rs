use serde::Deserialize;
use serde::Serialize;

/// A concrete downloadable size for a browsed model, such as `llama3.2:1b`.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ModelSize {
    /// Name a consumer pulls to install this variant.
    pub name: String,
    /// Human label, such as `1B` or `Q4_0`.
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}
