use serde::Deserialize;
use serde::Serialize;

use super::ContextBreakdown;
use super::ContextSource;
use super::ContextStatus;

/// How much of a model's context a request spends, and what that leaves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsage {
    pub model: String,
    pub used: u64,
    pub limit: Option<u64>,
    pub reserved: u32,
    pub threshold: Option<u64>,
    pub remaining: Option<u64>,
    pub estimated: bool,
    pub incomplete: bool,
    pub source: ContextSource,
    pub status: ContextStatus,
    pub breakdown: ContextBreakdown,
    pub revision: u64,
    pub compacted_messages: usize,
    pub updated_at: String,
    pub reason: Option<String>,
}
