use crate::cost::{ModelPricing, PricingUnit, TaskCategory};

/// The shipped pricing table: published list prices, with a quality and speed
/// score for each model.
///
/// `cost_per_unit` is a single number, so it cannot carry the input/output
/// split every text model has; the output rate is the one that dominates a
/// generation workload and is the conservative choice.
pub fn default_pricing() -> Vec<ModelPricing> {
    vec![
        ModelPricing {
            provider: "anthropic".into(),
            model: "claude-opus-5".into(),
            category: TaskCategory::Text,
            cost_per_unit: 25.0,
            unit: PricingUnit::PerMillionTokens,
            quality_score: 0.98,
            speed_score: 0.7,
            local_available: false,
        },
        ModelPricing {
            provider: "anthropic".into(),
            model: "claude-sonnet-5".into(),
            category: TaskCategory::Text,
            cost_per_unit: 10.0,
            unit: PricingUnit::PerMillionTokens,
            quality_score: 0.94,
            speed_score: 0.85,
            local_available: false,
        },
        ModelPricing {
            provider: "anthropic".into(),
            model: "claude-haiku-4-5".into(),
            category: TaskCategory::Text,
            cost_per_unit: 5.0,
            unit: PricingUnit::PerMillionTokens,
            quality_score: 0.85,
            speed_score: 0.95,
            local_available: false,
        },
        ModelPricing {
            provider: "openai".into(),
            model: "gpt-5.4".into(),
            category: TaskCategory::Text,
            cost_per_unit: 10.0,
            unit: PricingUnit::PerMillionTokens,
            quality_score: 0.95,
            speed_score: 0.8,
            local_available: false,
        },
        ModelPricing {
            provider: "google".into(),
            model: "gemini-2.5-pro".into(),
            category: TaskCategory::Text,
            cost_per_unit: 1.25,
            unit: PricingUnit::PerMillionTokens,
            quality_score: 0.90,
            speed_score: 0.9,
            local_available: false,
        },
        ModelPricing {
            provider: "google".into(),
            model: "gemini-2.5-flash".into(),
            category: TaskCategory::Text,
            cost_per_unit: 0.0,
            unit: PricingUnit::Free,
            quality_score: 0.85,
            speed_score: 0.95,
            local_available: false,
        },
        ModelPricing {
            provider: "ollama".into(),
            model: "llama-3.3-70b".into(),
            category: TaskCategory::Text,
            cost_per_unit: 0.0,
            unit: PricingUnit::Free,
            quality_score: 0.82,
            speed_score: 0.3,
            local_available: true,
        },
        ModelPricing {
            provider: "fal".into(),
            model: "flux-2-pro".into(),
            category: TaskCategory::Image,
            cost_per_unit: 0.03,
            unit: PricingUnit::PerImage,
            quality_score: 0.95,
            speed_score: 0.8,
            local_available: false,
        },
        ModelPricing {
            provider: "fal".into(),
            model: "flux-2-schnell".into(),
            category: TaskCategory::Image,
            cost_per_unit: 0.015,
            unit: PricingUnit::PerImage,
            quality_score: 0.85,
            speed_score: 0.95,
            local_available: false,
        },
        ModelPricing {
            provider: "openai".into(),
            model: "gpt-image-1.5".into(),
            category: TaskCategory::Image,
            cost_per_unit: 0.04,
            unit: PricingUnit::PerImage,
            quality_score: 0.95,
            speed_score: 0.7,
            local_available: false,
        },
        ModelPricing {
            provider: "local".into(),
            model: "sdxl-comfyui".into(),
            category: TaskCategory::Image,
            cost_per_unit: 0.0,
            unit: PricingUnit::Free,
            quality_score: 0.80,
            speed_score: 0.4,
            local_available: true,
        },
        ModelPricing {
            provider: "fish_audio".into(),
            model: "fish-s1".into(),
            category: TaskCategory::Voice,
            cost_per_unit: 0.000015,
            unit: PricingUnit::PerCharacter,
            quality_score: 0.92,
            speed_score: 0.9,
            local_available: false,
        },
        ModelPricing {
            provider: "elevenlabs".into(),
            model: "eleven-v3".into(),
            category: TaskCategory::Voice,
            cost_per_unit: 0.00003,
            unit: PricingUnit::PerCharacter,
            quality_score: 0.95,
            speed_score: 0.85,
            local_available: false,
        },
        ModelPricing {
            provider: "local".into(),
            model: "xtts-v2".into(),
            category: TaskCategory::Voice,
            cost_per_unit: 0.0,
            unit: PricingUnit::Free,
            quality_score: 0.70,
            speed_score: 0.3,
            local_available: true,
        },
        ModelPricing {
            provider: "suno".into(),
            model: "suno-v5".into(),
            category: TaskCategory::Music,
            cost_per_unit: 0.05,
            unit: PricingUnit::PerSecondAudio,
            quality_score: 0.95,
            speed_score: 0.7,
            local_available: false,
        },
        ModelPricing {
            provider: "local".into(),
            model: "musicgen-large".into(),
            category: TaskCategory::Music,
            cost_per_unit: 0.0,
            unit: PricingUnit::Free,
            quality_score: 0.60,
            speed_score: 0.2,
            local_available: true,
        },
        ModelPricing {
            provider: "tripo".into(),
            model: "tripo-v2".into(),
            category: TaskCategory::Model3D,
            cost_per_unit: 0.10,
            unit: PricingUnit::Per3DModel,
            quality_score: 0.90,
            speed_score: 0.7,
            local_available: false,
        },
        ModelPricing {
            provider: "meshy".into(),
            model: "meshy-6".into(),
            category: TaskCategory::Model3D,
            cost_per_unit: 0.15,
            unit: PricingUnit::Per3DModel,
            quality_score: 0.88,
            speed_score: 0.6,
            local_available: false,
        },
        ModelPricing {
            provider: "local".into(),
            model: "triposr".into(),
            category: TaskCategory::Model3D,
            cost_per_unit: 0.0,
            unit: PricingUnit::Free,
            quality_score: 0.65,
            speed_score: 0.3,
            local_available: true,
        },
        ModelPricing {
            provider: "replicate".into(),
            model: "kling-v2".into(),
            category: TaskCategory::Video,
            cost_per_unit: 0.50,
            unit: PricingUnit::PerVideoSecond,
            quality_score: 0.85,
            speed_score: 0.5,
            local_available: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_shipped_model_id_carries_a_date_suffix() {
        // `claude-opus-4-6-20250514` shipped as a default for months. No
        // Anthropic model id has a date suffix, so every call it was used for
        // would have been rejected.
        for entry in default_pricing() {
            if entry.provider != "anthropic" {
                continue;
            }
            let tail = entry.model.rsplit('-').next().unwrap_or("");
            assert!(
                !(tail.len() == 8 && tail.chars().all(|character| character.is_ascii_digit())),
                "{} looks like it has a date suffix",
                entry.model
            );
        }
    }

    #[test]
    fn the_table_holds_every_shipped_model() {
        let pricing = default_pricing();
        assert_eq!(pricing.len(), 20);

        let opus = pricing
            .iter()
            .find(|entry| entry.model == "claude-opus-5")
            .unwrap();
        assert_eq!(opus.cost_per_unit, 25.0);
        assert_eq!(opus.unit, PricingUnit::PerMillionTokens);
        assert!((opus.quality_score - 0.98).abs() < f64::EPSILON);

        let flash = pricing
            .iter()
            .find(|entry| entry.model == "gemini-2.5-flash")
            .unwrap();
        assert_eq!(flash.unit, PricingUnit::Free);

        assert!(pricing.iter().filter(|entry| entry.local_available).count() >= 5);
    }

    #[test]
    fn every_model_is_filed_under_the_work_it_does() {
        let pricing = default_pricing();
        let category = |model: &str| {
            pricing
                .iter()
                .find(|entry| entry.model == model)
                .map(|entry| entry.category)
        };

        assert_eq!(category("claude-opus-5"), Some(TaskCategory::Text));
        assert_eq!(category("sdxl-comfyui"), Some(TaskCategory::Image));
        assert_eq!(category("xtts-v2"), Some(TaskCategory::Voice));
        assert_eq!(category("musicgen-large"), Some(TaskCategory::Music));
        assert_eq!(category("triposr"), Some(TaskCategory::Model3D));
        assert_eq!(category("kling-v2"), Some(TaskCategory::Video));
    }

    #[test]
    fn a_free_model_is_priced_at_nothing() {
        for entry in default_pricing() {
            if entry.unit == PricingUnit::Free {
                assert_eq!(
                    entry.cost_per_unit, 0.0,
                    "{} is Free but priced",
                    entry.model
                );
            }
        }
    }

    #[test]
    fn every_score_is_a_fraction() {
        for entry in default_pricing() {
            assert!(
                entry.quality_score > 0.0 && entry.quality_score <= 1.0,
                "{} has quality {}",
                entry.model,
                entry.quality_score
            );
            assert!(
                entry.speed_score > 0.0 && entry.speed_score <= 1.0,
                "{} has speed {}",
                entry.model,
                entry.speed_score
            );
        }
    }
}
