use crate::dataset::Finding;
use crate::lora::Screening;
use crate::quality::Quality;
use serde::Serialize;
use std::path::PathBuf;

/// A finished run: the adapter on disk and, when ComfyUI could be asked, how
/// far it beats the base it was trained from.
#[derive(Debug, Serialize)]
pub struct TrainOutcome {
    pub path: PathBuf,
    pub quality: Option<Quality>,
    pub dataset: Vec<Finding>,
    pub screening: Screening,
}
