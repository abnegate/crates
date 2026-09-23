use serde::{Deserialize, Serialize};

/// One task within a [`CostEstimate`](crate::cost::CostEstimate), and what it
/// is expected to cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CostLineItem {
    pub task: String,
    pub provider: String,
    pub model: String,
    pub quantity: u32,
    /// What one unit of `quantity` costs: one token, one image, one second.
    pub unit_cost: f64,
    pub total_cost: f64,
    pub is_local: bool,
}
