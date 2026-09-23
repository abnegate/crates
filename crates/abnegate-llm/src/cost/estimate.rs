use serde::{Deserialize, Serialize};

use crate::cost::{CostLineItem, CostStrategy};

/// What a batch of tasks is expected to cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    pub total_usd: f64,
    pub breakdown: Vec<CostLineItem>,
    pub strategy_used: CostStrategy,
    pub local_savings_usd: f64,
    pub quality_score: f64,
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
        };

        let json = serde_json::to_string(&estimate).unwrap();
        let roundtrip: CostEstimate = serde_json::from_str(&json).unwrap();

        assert!((roundtrip.total_usd - 5.0).abs() < f64::EPSILON);
        assert!((roundtrip.local_savings_usd - 2.0).abs() < f64::EPSILON);
        assert_eq!(roundtrip.strategy_used, CostStrategy::BestValue);
    }
}
