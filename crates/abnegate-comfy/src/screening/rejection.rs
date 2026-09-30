use serde::Serialize;

/// Why screening left an image out of a training set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Rejection {
    /// It is a near copy of an image already kept.
    Duplicate,
    /// It is much softer than the rest of the set.
    Blurred,
    /// Its shorter side is too small to reach the training resolution even
    /// upscaled.
    Small,
}
