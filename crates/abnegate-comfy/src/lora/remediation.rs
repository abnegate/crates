use crate::lora::RemediationOutcome;
use crate::screening::Rejection;
use serde::Serialize;

/// One target the pipeline tried to repair before selecting the training set.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Remediation {
    pub source_index: usize,
    pub filename: String,
    pub reason: Rejection,
    pub outcome: RemediationOutcome,
}
