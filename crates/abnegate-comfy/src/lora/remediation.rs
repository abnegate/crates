use crate::lora::RemediationOutcome;
use crate::screening::Rejection;
use serde::Serialize;

/// One target the pipeline tried to repair before selecting the training set.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Remediation {
    /// The image's position in the request.
    pub source_index: usize,
    /// The image's name, as submitted.
    pub filename: String,
    /// What screening first rejected it for.
    pub reason: Rejection,
    /// What the repair came to.
    pub outcome: RemediationOutcome,
}
