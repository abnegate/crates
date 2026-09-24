use crate::lora::Dropped;
use crate::lora::Remediation;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct Screening {
    pub kept: usize,
    pub dropped: Vec<Dropped>,
    pub attempted: Vec<Remediation>,
}
