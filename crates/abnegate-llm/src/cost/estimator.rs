use std::cmp::Ordering;

use crate::cost::{
    CostEstimate, CostLineItem, CostStrategy, ModelPricing, PricingUnit, TaskCategory, TaskSpec,
    default_pricing,
};
use crate::hardware::MachineProfile;

const LOCAL_SPEED_SCORE: f64 = 0.3;
const FREE_MODEL_VALUE_MULTIPLIER: f64 = 100.0;
const NO_LOCAL_MODEL: &str = "none";

/// Chooses models for tasks and says what the choice costs.
pub struct CostEstimator;

impl CostEstimator {
    /// A pricing table whose local entries are the ones `profile` can actually
    /// run, at $0, replacing the table's generic local entries.
    pub fn with_hardware(profile: &MachineProfile) -> Vec<ModelPricing> {
        let mut pricing = default_pricing();
        let recommended = &profile.recommended_models;

        let local = [
            &recommended.llm,
            &recommended.image,
            &recommended.voice,
            &recommended.music,
            &recommended.model3d,
            &recommended.embedding,
            &recommended.transcription,
        ];

        pricing.retain(|entry| !(entry.provider == "local" || entry.provider == "ollama"));

        for recommendation in local {
            if recommendation.model_name == NO_LOCAL_MODEL {
                continue;
            }
            pricing.push(ModelPricing {
                provider: "local".into(),
                model: recommendation.model_name.clone(),
                cost_per_unit: 0.0,
                unit: PricingUnit::Free,
                quality_score: recommendation.quality_score,
                speed_score: LOCAL_SPEED_SCORE,
                local_available: true,
            });
        }

        pricing
    }

    pub fn estimate_batch_cost(
        requests: &[TaskSpec],
        pricing: &[ModelPricing],
        strategy: CostStrategy,
    ) -> CostEstimate {
        let mut breakdown = Vec::new();
        let mut total_usd = 0.0;
        let mut total_quality = 0.0;
        let mut total_items = 0u32;
        let mut local_total = 0.0;

        for task in requests {
            let chosen = match &strategy {
                CostStrategy::CheapestPossible => Self::cheapest_for(&task.category, pricing),
                CostStrategy::BestQuality => Self::best_quality_for(&task.category, pricing),
                CostStrategy::BestValue => Self::best_value_for(&task.category, pricing),
                CostStrategy::LocalFirst => Self::local_first_for(&task.category, pricing),
                CostStrategy::Budget { .. } => Self::best_value_for(&task.category, pricing),
            };

            if let Some(model) = chosen {
                let unit_cost = effective_cost(model);
                let line_total = unit_cost * f64::from(task.quantity);
                total_usd += line_total;
                total_quality += model.quality_score * f64::from(task.quantity);
                total_items += task.quantity;

                breakdown.push(line_item(task, model, unit_cost, line_total));
            }

            if let Some(local) = Self::cheapest_local_for(&task.category, pricing) {
                local_total += effective_cost(local) * f64::from(task.quantity);
            }
        }

        let local_savings = total_usd - local_total;
        let estimate = CostEstimate {
            total_usd,
            breakdown,
            strategy_used: strategy.clone(),
            local_savings_usd: local_savings.max(0.0),
            quality_score: average_quality(total_quality, total_items),
        };

        match &strategy {
            CostStrategy::Budget { max_usd } => {
                Self::apply_budget_constraint(estimate, requests, pricing, *max_usd)
            }
            _ => estimate,
        }
    }

    pub fn cheapest_for<'a>(
        category: &TaskCategory,
        pricing: &'a [ModelPricing],
    ) -> Option<&'a ModelPricing> {
        matching_models(category, pricing).min_by(|a, b| compare_cost(a, b))
    }

    pub fn best_quality_for<'a>(
        category: &TaskCategory,
        pricing: &'a [ModelPricing],
    ) -> Option<&'a ModelPricing> {
        matching_models(category, pricing).max_by(|a, b| compare_quality(a, b))
    }

    pub fn best_value_for<'a>(
        category: &TaskCategory,
        pricing: &'a [ModelPricing],
    ) -> Option<&'a ModelPricing> {
        matching_models(category, pricing).max_by(|a, b| {
            value_score(a)
                .partial_cmp(&value_score(b))
                .unwrap_or(Ordering::Equal)
        })
    }

    pub fn local_first_for<'a>(
        category: &TaskCategory,
        pricing: &'a [ModelPricing],
    ) -> Option<&'a ModelPricing> {
        matching_models(category, pricing)
            .filter(|model| model.local_available)
            .max_by(|a, b| compare_quality(a, b))
            .or_else(|| Self::best_value_for(category, pricing))
    }

    fn cheapest_local_for<'a>(
        category: &TaskCategory,
        pricing: &'a [ModelPricing],
    ) -> Option<&'a ModelPricing> {
        matching_models(category, pricing)
            .filter(|model| model.local_available)
            .min_by(|a, b| compare_cost(a, b))
    }

    fn apply_budget_constraint(
        mut estimate: CostEstimate,
        requests: &[TaskSpec],
        pricing: &[ModelPricing],
        max_usd: f64,
    ) -> CostEstimate {
        if estimate.total_usd <= max_usd {
            return estimate;
        }

        let mut breakdown = Vec::new();
        let mut remaining = max_usd;
        let mut total_quality = 0.0;
        let mut total_items = 0u32;

        let mut by_quality: Vec<(usize, f64)> = requests
            .iter()
            .enumerate()
            .map(|(index, task)| {
                let quality = Self::best_quality_for(&task.category, pricing)
                    .map_or(0.0, |model| model.quality_score);
                (index, quality)
            })
            .collect();
        by_quality.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));

        for (index, _) in &by_quality {
            let task = &requests[*index];

            let affordable = matching_models(&task.category, pricing)
                .filter(|model| effective_cost(model) * f64::from(task.quantity) <= remaining)
                .max_by(|a, b| compare_quality(a, b));

            let chosen = match affordable {
                Some(model) => Some(model),
                None => Self::cheapest_local_for(&task.category, pricing),
            };

            if let Some(model) = chosen {
                let unit_cost = effective_cost(model);
                let line_total = unit_cost * f64::from(task.quantity);
                remaining -= line_total;
                total_quality += model.quality_score * f64::from(task.quantity);
                total_items += task.quantity;

                breakdown.push(line_item(task, model, unit_cost, line_total));
            }
        }

        estimate.total_usd = breakdown.iter().map(|item| item.total_cost).sum();
        estimate.quality_score = average_quality(total_quality, total_items);
        estimate.breakdown = breakdown;
        estimate
    }
}

fn line_item(
    task: &TaskSpec,
    model: &ModelPricing,
    unit_cost: f64,
    total_cost: f64,
) -> CostLineItem {
    CostLineItem {
        task: task.label.clone(),
        provider: model.provider.clone(),
        model: model.model.clone(),
        quantity: task.quantity,
        unit_cost,
        total_cost,
        is_local: model.local_available && model.unit == PricingUnit::Free,
    }
}

fn average_quality(total_quality: f64, total_items: u32) -> f64 {
    if total_items == 0 {
        return 0.0;
    }
    total_quality / f64::from(total_items)
}

fn compare_cost(a: &ModelPricing, b: &ModelPricing) -> Ordering {
    effective_cost(a)
        .partial_cmp(&effective_cost(b))
        .unwrap_or(Ordering::Equal)
}

fn compare_quality(a: &ModelPricing, b: &ModelPricing) -> Ordering {
    a.quality_score
        .partial_cmp(&b.quality_score)
        .unwrap_or(Ordering::Equal)
}

fn effective_cost(model: &ModelPricing) -> f64 {
    if model.unit == PricingUnit::Free {
        0.0
    } else {
        model.cost_per_unit
    }
}

fn value_score(model: &ModelPricing) -> f64 {
    let cost = effective_cost(model);
    if cost <= 0.0 {
        return model.quality_score * FREE_MODEL_VALUE_MULTIPLIER;
    }
    model.quality_score / cost
}

fn matching_models<'a>(
    category: &TaskCategory,
    pricing: &'a [ModelPricing],
) -> impl Iterator<Item = &'a ModelPricing> {
    let category = category.clone();
    pricing
        .iter()
        .filter(move |model| matches_category(model, &category))
}

fn matches_category(model: &ModelPricing, category: &TaskCategory) -> bool {
    match category {
        TaskCategory::Text => {
            matches!(
                model.unit,
                PricingUnit::PerMillionTokens | PricingUnit::Free
            ) && is_text_provider(&model.provider)
        }
        TaskCategory::Image => {
            matches!(model.unit, PricingUnit::PerImage | PricingUnit::Free)
                && is_image_provider(&model.provider)
        }
        TaskCategory::Voice => {
            matches!(model.unit, PricingUnit::PerCharacter | PricingUnit::Free)
                && is_voice_provider(&model.provider)
        }
        TaskCategory::Music => {
            matches!(model.unit, PricingUnit::PerSecondAudio | PricingUnit::Free)
                && is_music_provider(&model.provider)
        }
        TaskCategory::Model3D => {
            matches!(model.unit, PricingUnit::Per3DModel | PricingUnit::Free)
                && is_model3d_provider(&model.provider)
        }
        TaskCategory::Video => {
            matches!(model.unit, PricingUnit::PerVideoSecond | PricingUnit::Free)
        }
    }
}

fn is_text_provider(provider: &str) -> bool {
    matches!(
        provider,
        "anthropic" | "openai" | "google" | "ollama" | "local" | "kimi"
    )
}

fn is_image_provider(provider: &str) -> bool {
    matches!(
        provider,
        "fal" | "openai" | "stability" | "local" | "replicate"
    )
}

fn is_voice_provider(provider: &str) -> bool {
    matches!(provider, "fish_audio" | "elevenlabs" | "local")
}

fn is_music_provider(provider: &str) -> bool {
    matches!(provider, "suno" | "local")
}

fn is_model3d_provider(provider: &str) -> bool {
    matches!(provider, "tripo" | "meshy" | "replicate" | "local")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(label: &str, category: TaskCategory, quantity: u32) -> TaskSpec {
        TaskSpec {
            label: label.into(),
            category,
            quantity,
        }
    }

    #[test]
    fn the_cheapest_image_model_is_a_free_local_one() {
        let pricing = default_pricing();
        let cheapest = CostEstimator::cheapest_for(&TaskCategory::Image, &pricing).unwrap();
        assert_eq!(cheapest.model, "sdxl-comfyui");
        assert!(cheapest.local_available);
    }

    #[test]
    fn the_best_image_model_is_one_of_the_two_top_scorers() {
        let pricing = default_pricing();
        let best = CostEstimator::best_quality_for(&TaskCategory::Image, &pricing).unwrap();
        assert!(best.quality_score >= 0.95);
        assert!(best.model == "flux-2-pro" || best.model == "gpt-image-1.5");
    }

    #[test]
    fn the_best_value_voice_model_is_free_or_the_cheap_paid_one() {
        let pricing = default_pricing();
        let best = CostEstimator::best_value_for(&TaskCategory::Voice, &pricing).unwrap();
        assert!(best.local_available || best.provider == "fish_audio");
    }

    #[test]
    fn every_category_has_a_cheapest_model() {
        let pricing = default_pricing();
        for category in [
            TaskCategory::Text,
            TaskCategory::Image,
            TaskCategory::Voice,
            TaskCategory::Music,
            TaskCategory::Model3D,
            TaskCategory::Video,
        ] {
            let cheapest = CostEstimator::cheapest_for(&category, &pricing);
            assert!(cheapest.is_some(), "{category:?} has no model");
            assert!(!cheapest.unwrap().model.is_empty(), "{category:?}");
        }
    }

    #[test]
    fn the_cheapest_text_voice_and_music_models_cost_nothing() {
        let pricing = default_pricing();
        for category in [TaskCategory::Text, TaskCategory::Voice, TaskCategory::Music] {
            let cheapest = CostEstimator::cheapest_for(&category, &pricing).unwrap();
            assert!(
                effective_cost(cheapest) == 0.0 || cheapest.local_available,
                "{category:?} picked {}",
                cheapest.model
            );
        }
    }

    #[test]
    fn every_category_has_a_best_quality_model() {
        let pricing = default_pricing();
        let expectations = [
            (TaskCategory::Text, 0.9),
            (TaskCategory::Voice, 0.92),
            (TaskCategory::Music, 0.5),
            (TaskCategory::Model3D, 0.65),
            (TaskCategory::Video, 0.0),
        ];

        for (category, floor) in expectations {
            let best = CostEstimator::best_quality_for(&category, &pricing).unwrap();
            assert!(best.quality_score > floor, "{category:?}");
        }
    }

    #[test]
    fn every_category_has_a_best_value_model() {
        let pricing = default_pricing();
        for category in [
            TaskCategory::Text,
            TaskCategory::Image,
            TaskCategory::Model3D,
        ] {
            let best = CostEstimator::best_value_for(&category, &pricing).unwrap();
            assert!(!best.provider.is_empty(), "{category:?}");
            assert!(!best.model.is_empty(), "{category:?}");
        }
    }

    #[test]
    fn local_first_picks_a_local_model_wherever_one_exists() {
        let pricing = default_pricing();
        for category in [
            TaskCategory::Text,
            TaskCategory::Image,
            TaskCategory::Voice,
            TaskCategory::Music,
            TaskCategory::Model3D,
            TaskCategory::Video,
        ] {
            let chosen = CostEstimator::local_first_for(&category, &pricing).unwrap();
            assert!(
                chosen.local_available,
                "{category:?} picked {}",
                chosen.model
            );
        }
    }

    #[test]
    fn local_first_falls_back_to_best_value_when_nothing_runs_locally() {
        let pricing = vec![ModelPricing {
            provider: "openai".into(),
            model: "gpt-image-1.5".into(),
            cost_per_unit: 0.04,
            unit: PricingUnit::PerImage,
            quality_score: 0.95,
            speed_score: 0.7,
            local_available: false,
        }];

        let chosen = CostEstimator::local_first_for(&TaskCategory::Image, &pricing).unwrap();

        assert!(!chosen.local_available);
        assert_eq!(chosen.model, "gpt-image-1.5");
    }

    #[test]
    fn no_tasks_cost_nothing() {
        let pricing = default_pricing();
        let estimate = CostEstimator::estimate_batch_cost(&[], &pricing, CostStrategy::BestValue);
        assert_eq!(estimate.total_usd, 0.0);
        assert_eq!(estimate.quality_score, 0.0);
        assert!(estimate.breakdown.is_empty());
    }

    #[test]
    fn one_line_item_is_reported_per_task() {
        let pricing = default_pricing();
        let tasks = [
            task("Env art", TaskCategory::Image, 5),
            task("Dialogue", TaskCategory::Voice, 24),
            task("Music", TaskCategory::Music, 4),
            task("3D props", TaskCategory::Model3D, 2),
        ];

        let estimate =
            CostEstimator::estimate_batch_cost(&tasks, &pricing, CostStrategy::BestValue);

        assert_eq!(estimate.breakdown.len(), 4);
        for item in &estimate.breakdown {
            assert!(item.quantity > 0);
            assert!(!item.provider.is_empty());
            assert!(!item.model.is_empty());
        }
    }

    #[test]
    fn a_batch_of_paid_models_costs_something_and_scores_something() {
        let pricing = default_pricing();
        let tasks = [
            task("Image 1", TaskCategory::Image, 3),
            task("Image 2", TaskCategory::Image, 2),
            task("Voice", TaskCategory::Voice, 5),
        ];

        let estimate =
            CostEstimator::estimate_batch_cost(&tasks, &pricing, CostStrategy::BestQuality);

        assert_eq!(estimate.breakdown.len(), 3);
        assert!(estimate.total_usd > 0.0);
        assert!(estimate.quality_score > 0.0);
    }

    #[test]
    fn every_category_is_costed_under_the_cheapest_strategy() {
        let pricing = default_pricing();
        let tasks = [
            task("Text", TaskCategory::Text, 1),
            task("Image", TaskCategory::Image, 1),
            task("Voice", TaskCategory::Voice, 1),
            task("Music", TaskCategory::Music, 1),
            task("3D", TaskCategory::Model3D, 1),
            task("Video", TaskCategory::Video, 1),
        ];

        let estimate =
            CostEstimator::estimate_batch_cost(&tasks, &pricing, CostStrategy::CheapestPossible);

        assert_eq!(estimate.breakdown.len(), 6);
    }

    #[test]
    fn running_locally_is_reported_as_a_saving() {
        let pricing = default_pricing();
        let tasks = [
            task("Images", TaskCategory::Image, 5),
            task("Voice", TaskCategory::Voice, 10),
        ];

        let estimate =
            CostEstimator::estimate_batch_cost(&tasks, &pricing, CostStrategy::BestQuality);

        assert!(estimate.total_usd > 0.0);
        assert!(estimate.local_savings_usd > 0.0);
    }

    #[test]
    fn a_budget_that_covers_everything_changes_nothing() {
        let pricing = default_pricing();
        let tasks = [task("Image", TaskCategory::Image, 1)];

        let estimate = CostEstimator::estimate_batch_cost(
            &tasks,
            &pricing,
            CostStrategy::Budget { max_usd: 100.0 },
        );

        assert!(estimate.total_usd <= 100.0);
        assert_eq!(estimate.breakdown.len(), 1);
    }

    #[test]
    fn a_budget_is_never_exceeded() {
        let pricing = default_pricing();
        let tasks = [
            task("Music", TaskCategory::Music, 4),
            task("Voice", TaskCategory::Voice, 10),
            task("Images", TaskCategory::Image, 5),
        ];

        let estimate = CostEstimator::estimate_batch_cost(
            &tasks,
            &pricing,
            CostStrategy::Budget { max_usd: 3.0 },
        );

        assert!(estimate.total_usd <= 3.0 + f64::EPSILON);
        assert!(!estimate.breakdown.is_empty());
    }

    #[test]
    fn a_budget_too_small_for_anything_paid_falls_back_to_local_models() {
        let pricing = default_pricing();
        let tasks = [
            task("A", TaskCategory::Image, 100),
            task("B", TaskCategory::Voice, 1000),
            task("C", TaskCategory::Music, 50),
        ];

        let estimate = CostEstimator::estimate_batch_cost(
            &tasks,
            &pricing,
            CostStrategy::Budget { max_usd: 0.01 },
        );

        assert!(estimate.total_usd <= 0.01 + f64::EPSILON);
        assert!(estimate.breakdown.iter().all(|item| item.is_local));
    }

    #[test]
    fn a_hardware_profile_replaces_the_generic_local_entries() {
        let profile = MachineProfile::from_preset("m4-max-64").unwrap();
        let pricing = CostEstimator::with_hardware(&profile);

        let local: Vec<&ModelPricing> = pricing
            .iter()
            .filter(|entry| entry.local_available)
            .collect();
        assert!(!local.is_empty());
        for entry in &local {
            assert_eq!(entry.cost_per_unit, 0.0);
            assert_eq!(entry.unit, PricingUnit::Free);
            assert!(entry.quality_score > 0.0);
        }

        let llm = local
            .iter()
            .find(|entry| entry.model.contains("qwen2.5:72b"))
            .unwrap();
        assert!((llm.quality_score - 0.92).abs() < f64::EPSILON);
    }

    #[test]
    fn a_smaller_machine_gets_a_lower_scoring_local_model() {
        let small =
            CostEstimator::with_hardware(&MachineProfile::from_preset("5900x-3080ti").unwrap());
        let large =
            CostEstimator::with_hardware(&MachineProfile::from_preset("m4-max-64").unwrap());

        let small_llm = small
            .iter()
            .find(|entry| entry.local_available && entry.model.contains("qwen2.5:14b"))
            .unwrap();
        let large_llm = large
            .iter()
            .find(|entry| entry.local_available && entry.model.contains("qwen2.5:72b"))
            .unwrap();

        assert!((small_llm.quality_score - 0.75).abs() < f64::EPSILON);
        assert!(large_llm.quality_score > small_llm.quality_score);
    }

    #[test]
    fn local_first_uses_the_hardware_entries() {
        let profile = MachineProfile::from_preset("m4-max-64").unwrap();
        let pricing = CostEstimator::with_hardware(&profile);
        let recommended = &profile.recommended_models;

        let chosen = CostEstimator::local_first_for(&TaskCategory::Image, &pricing).unwrap();

        assert!(chosen.local_available);
        assert_eq!(chosen.cost_per_unit, 0.0);
        assert!(
            [
                &recommended.llm,
                &recommended.image,
                &recommended.voice,
                &recommended.music,
                &recommended.model3d,
                &recommended.embedding,
                &recommended.transcription,
            ]
            .iter()
            .any(|recommendation| recommendation.model_name == chosen.model),
            "{} is not a model this machine was told it could run",
            chosen.model
        );
    }

    #[test]
    fn a_machine_that_cannot_run_a_modality_contributes_no_entry_for_it() {
        let profile = MachineProfile::from_preset("cpu-only").unwrap();
        let pricing = CostEstimator::with_hardware(&profile);

        assert_eq!(profile.recommended_models.image.model_name, NO_LOCAL_MODEL);
        assert!(
            !pricing.iter().any(|entry| entry.model == NO_LOCAL_MODEL),
            "a machine's unavailable modality must not become a free model"
        );
    }
}
