use serde::Deserialize;
use serde::Serialize;

/// One task within a [`CostEstimate`](crate::cost::CostEstimate), and what it
/// is expected to cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CostLineItem {
    /// The task's label.
    pub task: String,
    /// Who serves the model, `local` for one on this machine.
    pub provider: String,
    /// The model assigned.
    pub model: String,
    /// How many units the task needs: tokens, images, seconds and so on.
    pub quantity: u32,
    /// What one unit of `quantity` costs: one token, one image, one second.
    pub unit_cost: f64,
    /// What the whole task costs, in US dollars.
    pub total_cost: f64,
    /// Whether it runs free on this machine.
    pub is_local: bool,
}
