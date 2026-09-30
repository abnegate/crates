use crate::lora::Dropped;
use crate::lora::Remediation;
use serde::Serialize;

/// What screening made of the submitted images.
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct Screening {
    /// How many images were trained on.
    pub kept: usize,
    /// The images left out, and why.
    pub dropped: Vec<Dropped>,
    /// The images an upscale was tried on, and how each fared.
    pub attempted: Vec<Remediation>,
}
