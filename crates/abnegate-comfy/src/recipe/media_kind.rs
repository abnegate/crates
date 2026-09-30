use serde::Deserialize;

/// What a recipe produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MediaKind {
    /// Still images.
    Image,
    /// Video.
    Video,
}
