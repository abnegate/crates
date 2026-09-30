use crate::catalog::capability::ModelCapability;
use crate::catalog::details::ModelDetails;
use crate::catalog::size::ModelSize;
use serde::Deserialize;
use serde::Serialize;

/// One model as a catalogue describes it.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct ModelEntry {
    /// The identifier a consumer installs or calls the model by.
    pub name: String,
    /// A friendlier name, when the catalogue gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The download size, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The content digest of the download.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// When the model was last updated, as the catalogue writes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<String>,
    /// What the model is, in the catalogue's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Who published it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// The model's page in the catalogue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// How many times it has been downloaded or pulled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<u64>,
    /// How many people have liked it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub likes: Option<u64>,
    /// The catalogue's tags for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// What the catalogue says it is good for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_cases: Option<Vec<String>>,
    /// Capabilities declared by provider metadata; absent when unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<ModelCapability>>,
    /// Distinct downloadable sizes when a catalogue entry ships more than one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sizes: Option<Vec<ModelSize>>,
    /// Format, family and runtime facts, when the catalogue gives any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ModelDetails>,
}
