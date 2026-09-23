use serde::Deserialize;
use serde::Serialize;

/// Parameter-count bucket a browse is restricted to.
///
/// Serialised in snake case; `xl` is still read as
/// [`ModelSizeFilter::ExtraLarge`].
#[derive(Debug, Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelSizeFilter {
    #[default]
    All,
    Small,
    Medium,
    Large,
    #[serde(alias = "xl")]
    ExtraLarge,
}
