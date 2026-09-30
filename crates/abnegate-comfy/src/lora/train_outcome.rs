use crate::dataset::Finding;
use crate::lora::Screening;
use crate::quality::Quality;
use serde::Serialize;
use std::path::PathBuf;

/// A finished run: the adapter on disk and, when ComfyUI could be asked, how
/// far it beats the base it was trained from.
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct TrainOutcome {
    /// Where the adapter was installed.
    pub path: PathBuf,
    /// How it scored against its base, when ComfyUI could score it.
    pub quality: Option<Quality>,
    /// Advisories about the training set.
    pub dataset: Vec<Finding>,
    /// Which images were trained on, and what became of the rest.
    pub screening: Screening,
}
