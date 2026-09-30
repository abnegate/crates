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
    /// Every size, including models whose size is unknown. The default.
    #[default]
    All,
    /// Under 4 billion parameters.
    Small,
    /// From 4 up to 16 billion parameters.
    Medium,
    /// From 16 up to 40 billion parameters.
    Large,
    /// 40 billion parameters or more.
    #[serde(alias = "xl")]
    ExtraLarge,
}
