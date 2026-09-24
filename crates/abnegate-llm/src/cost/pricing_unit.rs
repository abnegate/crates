use serde::{Deserialize, Serialize};

const TOKENS_PER_MILLION: f64 = 1_000_000.0;

/// What a model's price is quoted per.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PricingUnit {
    PerMillionTokens,
    PerImage,
    PerSecondAudio,
    PerCharacter,
    Per3DModel,
    PerVideoSecond,
    Free,
}

impl PricingUnit {
    /// How many units of a [`TaskSpecification`](crate::cost::TaskSpecification) quantity one
    /// quoted price covers: a million for a per-million-token price, one for
    /// every other unit.
    pub fn quantity_per_price(self) -> f64 {
        match self {
            Self::PerMillionTokens => TOKENS_PER_MILLION,
            Self::PerImage
            | Self::PerSecondAudio
            | Self::PerCharacter
            | Self::Per3DModel
            | Self::PerVideoSecond
            | Self::Free => 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_round_trips() {
        let units = [
            PricingUnit::PerMillionTokens,
            PricingUnit::PerImage,
            PricingUnit::PerSecondAudio,
            PricingUnit::PerCharacter,
            PricingUnit::Per3DModel,
            PricingUnit::PerVideoSecond,
            PricingUnit::Free,
        ];

        for unit in &units {
            let json = serde_json::to_string(unit).unwrap();
            let roundtrip: PricingUnit = serde_json::from_str(&json).unwrap();
            assert_eq!(*unit, roundtrip);
        }
    }

    #[test]
    fn only_a_token_price_covers_more_than_one_unit() {
        assert_eq!(
            PricingUnit::PerMillionTokens.quantity_per_price(),
            1_000_000.0
        );
        assert_eq!(PricingUnit::PerImage.quantity_per_price(), 1.0);
        assert_eq!(PricingUnit::PerCharacter.quantity_per_price(), 1.0);
    }
}
