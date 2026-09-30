use serde::Serialize;

/// What a training set may be doing wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Concern {
    /// Fewer than five images.
    TooFew,
    /// The images are near copies of one shot.
    LowVariety,
    /// The images do not look like one subject.
    MixedSubjects,
    /// The subject holds the same pose in every image.
    LowPoseVariety,
}
