use serde::Deserialize;
use serde::Serialize;

use crate::cost::CostLineItem;
use crate::cost::CostStrategy;

/// What a batch of tasks is expected to cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CostEstimate {
    /// The whole batch's cost, in US dollars.
    pub total_usd: f64,
    /// Each assigned task and what it costs.
    pub breakdown: Vec<CostLineItem>,
    /// The strategy the models were chosen under.
    pub strategy_used: CostStrategy,
    /// What running every task that has a local option locally would save
    /// against the chosen models. A task with no local option contributes
    /// nothing, because there is nothing to save it with.
    pub local_savings_usd: f64,
    /// The chosen models' quality scores, averaged over every unit of work
    /// assigned, from 0 to 1; 0 when nothing was assigned.
    pub quality_score: f64,
    /// The labels of the tasks no model was assigned to, because no model does
    /// that work or the budget could not cover one.
    #[serde(default)]
    pub unassigned: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let estimate = CostEstimate {
            total_usd: 5.0,
            breakdown: Vec::new(),
            strategy_used: CostStrategy::BestValue,
            local_savings_usd: 2.0,
            quality_score: 0.85,
            unassigned: vec!["Video".into()],
        };

        let json = serde_json::to_string(&estimate).unwrap();
        let roundtrip: CostEstimate = serde_json::from_str(&json).unwrap();

        assert!((roundtrip.total_usd - 5.0).abs() < f64::EPSILON);
        assert!((roundtrip.local_savings_usd - 2.0).abs() < f64::EPSILON);
        assert_eq!(roundtrip.strategy_used, CostStrategy::BestValue);
        assert_eq!(roundtrip.unassigned, vec!["Video".to_string()]);
    }
}
