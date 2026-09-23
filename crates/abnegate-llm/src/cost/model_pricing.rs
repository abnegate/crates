use serde::{Deserialize, Serialize};

use crate::cost::PricingUnit;

/// What one model costs, and how good and how fast it is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub provider: String,
    pub model: String,
    pub cost_per_unit: f64,
    pub unit: PricingUnit,
    pub quality_score: f64,
    pub speed_score: f64,
    pub local_available: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let pricing = ModelPricing {
            provider: "test".into(),
            model: "test-model".into(),
            cost_per_unit: 0.5,
            unit: PricingUnit::PerImage,
            quality_score: 0.9,
            speed_score: 0.8,
            local_available: false,
        };

        let json = serde_json::to_string(&pricing).unwrap();
        let roundtrip: ModelPricing = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.provider, "test");
        assert_eq!(roundtrip.model, "test-model");
        assert!((roundtrip.cost_per_unit - 0.5).abs() < f64::EPSILON);
        assert_eq!(roundtrip.unit, PricingUnit::PerImage);
        assert!(!roundtrip.local_available);
    }
}
