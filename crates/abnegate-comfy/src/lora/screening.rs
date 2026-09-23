use crate::lora::Dropped;
use crate::lora::Remediation;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Screening {
    pub kept: usize,
    pub dropped: Vec<Dropped>,
    pub attempted: Vec<Remediation>,
}
