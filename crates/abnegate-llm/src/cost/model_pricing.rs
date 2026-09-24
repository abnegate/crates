use serde::{Deserialize, Serialize};

use crate::cost::{PricingUnit, TaskCategory};

/// What one model costs, and how good and how fast it is.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelPricing {
    pub provider: String,
    pub model: String,
    /// The work this model does. A model is only offered for its own category.
    pub category: TaskCategory,
    /// The price quoted per [`ModelPricing::unit`].
    pub cost_per_unit: f64,
    pub unit: PricingUnit,
    pub quality_score: f64,
    pub speed_score: f64,
    pub local_available: bool,
}

impl ModelPricing {
    /// `model` from `provider`, doing `category` work at `cost_per_unit` per
    /// `unit`, with no quality or speed score and no local copy.
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        category: TaskCategory,
        cost_per_unit: f64,
        unit: PricingUnit,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            category,
            cost_per_unit,
            unit,
            quality_score: 0.0,
            speed_score: 0.0,
            local_available: false,
        }
    }

    /// Set how good the model's work is, from 0 to 1.
    pub fn with_quality_score(mut self, quality_score: f64) -> Self {
        self.quality_score = quality_score;
        self
    }

    /// Set how fast the model works, from 0 to 1.
    pub fn with_speed_score(mut self, speed_score: f64) -> Self {
        self.speed_score = speed_score;
        self
    }

    /// Set whether the model runs on this machine.
    pub fn with_local_available(mut self, local_available: bool) -> Self {
        self.local_available = local_available;
        self
    }

    /// What one unit of task quantity costs: one token under a per-million
    /// token price, one image under a per-image price.
    pub fn unit_cost(&self) -> f64 {
        if self.unit == PricingUnit::Free {
            return 0.0;
        }
        self.cost_per_unit / self.unit.quantity_per_price()
    }

    /// What `quantity` units of work cost on this model.
    pub fn cost_for(&self, quantity: u32) -> f64 {
        if self.unit == PricingUnit::Free {
            return 0.0;
        }
        self.cost_per_unit * f64::from(quantity) / self.unit.quantity_per_price()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pricing(cost_per_unit: f64, unit: PricingUnit) -> ModelPricing {
        ModelPricing::new(
            "test",
            "test-model",
            TaskCategory::Text,
            cost_per_unit,
            unit,
        )
        .with_quality_score(0.9)
        .with_speed_score(0.8)
    }

    #[test]
    fn new_scores_nothing_and_runs_remotely() {
        let pricing = ModelPricing::new(
            "openai",
            "gpt-image-1",
            TaskCategory::Image,
            0.04,
            PricingUnit::PerImage,
        );

        assert_eq!(pricing.provider, "openai");
        assert_eq!(pricing.model, "gpt-image-1");
        assert_eq!(pricing.category, TaskCategory::Image);
        assert_eq!(pricing.cost_per_unit, 0.04);
        assert_eq!(pricing.unit, PricingUnit::PerImage);
        assert_eq!((pricing.quality_score, pricing.speed_score), (0.0, 0.0));
        assert!(!pricing.local_available);
        assert!(pricing.with_local_available(true).local_available);
    }

    #[test]
    fn round_trips_through_json() {
        let pricing = ModelPricing::new(
            "test",
            "test-model",
            TaskCategory::Image,
            0.5,
            PricingUnit::PerImage,
        )
        .with_quality_score(0.9)
        .with_speed_score(0.8);

        let json = serde_json::to_string(&pricing).unwrap();
        let roundtrip: ModelPricing = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.provider, "test");
        assert_eq!(roundtrip.model, "test-model");
        assert_eq!(roundtrip.category, TaskCategory::Image);
        assert!((roundtrip.cost_per_unit - 0.5).abs() < f64::EPSILON);
        assert_eq!(roundtrip.unit, PricingUnit::PerImage);
        assert!(!roundtrip.local_available);
    }

    #[test]
    fn a_token_price_is_quoted_per_million() {
        let opus = pricing(25.0, PricingUnit::PerMillionTokens);

        assert_eq!(opus.cost_for(200_000), 5.0);
        assert_eq!(opus.cost_for(1_000_000), 25.0);
        assert_eq!(opus.unit_cost(), 0.000_025);
    }

    #[test]
    fn a_unit_price_is_quoted_per_unit() {
        let image = pricing(0.04, PricingUnit::PerImage);

        assert_eq!(image.cost_for(5), 0.2);
        assert_eq!(image.unit_cost(), 0.04);
    }

    #[test]
    fn a_free_model_costs_nothing_whatever_it_is_quoted_at() {
        let free = pricing(9.0, PricingUnit::Free);

        assert_eq!(free.cost_for(1_000), 0.0);
        assert_eq!(free.unit_cost(), 0.0);
    }
}
