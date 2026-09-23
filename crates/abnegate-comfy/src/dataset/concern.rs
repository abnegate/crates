use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Concern {
    TooFew,
    LowVariety,
    MixedSubjects,
    LowPoseVariety,
}
