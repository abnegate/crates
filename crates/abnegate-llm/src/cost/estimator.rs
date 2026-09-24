use std::cmp::Ordering;

use crate::cost::{
    CostEstimate, CostLineItem, CostStrategy, ModelPricing, PricingUnit, TaskCategory,
    TaskSpecification, default_pricing,
};
use crate::hardware::{MachineProfile, NO_LOCAL_MODEL};

const LOCAL_PROVIDER: &str = "local";
const LOCAL_SPEED_SCORE: f64 = 0.3;
const FREE_MODEL_VALUE_MULTIPLIER: f64 = 100.0;

/// Chooses models for tasks and says what the choice costs.
pub struct CostEstimator;

impl CostEstimator {
    /// A pricing table whose local entries are the ones `profile` can actually
    /// run, at $0, replacing the table's generic local entries.
    ///
    /// Each recommendation is filed under the category of the slot it came
    /// from, so the machine's embedding model is never offered for text and
    /// its image model never for voice.
    pub fn with_hardware(profile: &MachineProfile) -> Vec<ModelPricing> {
        let mut pricing = default_pricing();
        let recommended = &profile.recommended_models;

        let local = [
            (TaskCategory::Text, &recommended.llm),
            (TaskCategory::Image, &recommended.image),
            (TaskCategory::Voice, &recommended.voice),
            (TaskCategory::Music, &recommended.music),
            (TaskCategory::Model3D, &recommended.model3d),
            (TaskCategory::Embedding, &recommended.embedding),
            (TaskCategory::Transcription, &recommended.transcription),
        ];

        pricing.retain(|entry| !entry.local_available);

        for (category, recommendation) in local {
            if recommendation.model_name == NO_LOCAL_MODEL {
                continue;
            }
            pricing.push(ModelPricing {
                provider: LOCAL_PROVIDER.into(),
                model: recommendation.model_name.clone(),
                category,
                cost_per_unit: 0.0,
                unit: PricingUnit::Free,
                quality_score: recommendation.quality_score,
                speed_score: LOCAL_SPEED_SCORE,
                local_available: true,
            });
        }

        pricing
    }

    /// Assign a model to every task under `strategy` and total the result.
    ///
    /// A task no model can do, or one a budget cannot cover and no local model
    /// can take, is listed in [`CostEstimate::unassigned`] rather than dropped.
    pub fn estimate_batch_cost(
        requests: &[TaskSpecification],
        pricing: &[ModelPricing],
        strategy: CostStrategy,
    ) -> CostEstimate {
        let assignments: Vec<Option<&ModelPricing>> = requests
            .iter()
            .map(|task| Self::choose(&strategy, task.category, pricing))
            .collect();
        let estimate = assemble(requests, &assignments, pricing, strategy.clone());

        match strategy {
            CostStrategy::Budget { maximum_usd } if estimate.total_usd > maximum_usd => {
                Self::apply_budget_constraint(requests, pricing, strategy, maximum_usd)
            }
            _ => estimate,
        }
    }

    pub fn cheapest_for(category: TaskCategory, pricing: &[ModelPricing]) -> Option<&ModelPricing> {
        matching_models(category, pricing).min_by(|a, b| compare_cost(a, b))
    }

    pub fn best_quality_for(
        category: TaskCategory,
        pricing: &[ModelPricing],
    ) -> Option<&ModelPricing> {
        matching_models(category, pricing).max_by(|a, b| compare_quality(a, b))
    }

    pub fn best_value_for(
        category: TaskCategory,
        pricing: &[ModelPricing],
    ) -> Option<&ModelPricing> {
        matching_models(category, pricing).max_by(|a, b| {
            value_score(a)
                .partial_cmp(&value_score(b))
                .unwrap_or(Ordering::Equal)
        })
    }

    pub fn local_first_for(
        category: TaskCategory,
        pricing: &[ModelPricing],
    ) -> Option<&ModelPricing> {
        matching_models(category, pricing)
            .filter(|model| model.local_available)
            .max_by(|a, b| compare_quality(a, b))
            .or_else(|| Self::best_value_for(category, pricing))
    }

    fn choose<'a>(
        strategy: &CostStrategy,
        category: TaskCategory,
        pricing: &'a [ModelPricing],
    ) -> Option<&'a ModelPricing> {
        match strategy {
            CostStrategy::CheapestPossible => Self::cheapest_for(category, pricing),
            CostStrategy::BestQuality => Self::best_quality_for(category, pricing),
            CostStrategy::BestValue | CostStrategy::Budget { .. } => {
                Self::best_value_for(category, pricing)
            }
            CostStrategy::LocalFirst => Self::local_first_for(category, pricing),
        }
    }

    fn cheapest_local_for(
        category: TaskCategory,
        pricing: &[ModelPricing],
    ) -> Option<&ModelPricing> {
        matching_models(category, pricing)
            .filter(|model| model.local_available)
            .min_by(|a, b| compare_cost(a, b))
    }

    /// Spend the budget on the tasks whose best model scores highest first,
    /// giving each the best model it can still afford, then the cheapest
    /// local model, and otherwise leaving it unassigned.
    fn apply_budget_constraint(
        requests: &[TaskSpecification],
        pricing: &[ModelPricing],
        strategy: CostStrategy,
        maximum_usd: f64,
    ) -> CostEstimate {
        let mut by_quality: Vec<(usize, f64)> = requests
            .iter()
            .enumerate()
            .map(|(index, task)| {
                let quality = Self::best_quality_for(task.category, pricing)
                    .map_or(0.0, |model| model.quality_score);
                (index, quality)
            })
            .collect();
        by_quality.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));

        let mut remaining = maximum_usd;
        let mut assignments: Vec<Option<&ModelPricing>> = vec![None; requests.len()];
        for (index, _) in by_quality {
            let task = &requests[index];
            let chosen = matching_models(task.category, pricing)
                .filter(|model| model.cost_for(task.quantity) <= remaining)
                .max_by(|a, b| compare_quality(a, b))
                .or_else(|| Self::cheapest_local_for(task.category, pricing));

            if let Some(model) = chosen {
                remaining -= model.cost_for(task.quantity);
            }
            assignments[index] = chosen;
        }

        assemble(requests, &assignments, pricing, strategy)
    }
}

fn assemble(
    requests: &[TaskSpecification],
    assignments: &[Option<&ModelPricing>],
    pricing: &[ModelPricing],
    strategy: CostStrategy,
) -> CostEstimate {
    let mut breakdown = Vec::new();
    let mut unassigned = Vec::new();
    let mut total_usd = 0.0;
    let mut total_quality = 0.0;
    let mut total_items = 0_u64;
    let mut local_savings_usd = 0.0;

    for (task, assignment) in requests.iter().zip(assignments) {
        let Some(model) = assignment else {
            unassigned.push(task.label.clone());
            continue;
        };

        let total_cost = model.cost_for(task.quantity);
        total_usd += total_cost;
        total_quality += model.quality_score * f64::from(task.quantity);
        total_items += u64::from(task.quantity);

        if let Some(local) = CostEstimator::cheapest_local_for(task.category, pricing) {
            local_savings_usd += (total_cost - local.cost_for(task.quantity)).max(0.0);
        }

        breakdown.push(CostLineItem {
            task: task.label.clone(),
            provider: model.provider.clone(),
            model: model.model.clone(),
            quantity: task.quantity,
            unit_cost: model.unit_cost(),
            total_cost,
            is_local: model.local_available && model.unit == PricingUnit::Free,
        });
    }

    CostEstimate {
        total_usd,
        breakdown,
        strategy_used: strategy,
        local_savings_usd,
        quality_score: average_quality(total_quality, total_items),
        unassigned,
    }
}

fn average_quality(total_quality: f64, total_items: u64) -> f64 {
    if total_items == 0 {
        return 0.0;
    }
    total_quality / total_items as f64
}

fn compare_cost(a: &ModelPricing, b: &ModelPricing) -> Ordering {
    a.unit_cost()
        .partial_cmp(&b.unit_cost())
        .unwrap_or(Ordering::Equal)
}

fn compare_quality(a: &ModelPricing, b: &ModelPricing) -> Ordering {
    a.quality_score
        .partial_cmp(&b.quality_score)
        .unwrap_or(Ordering::Equal)
}

/// Quality per quoted dollar. Models compete only within one category, so
/// every model in a comparison is quoted in the same unit and the quote itself
/// is the fair denominator.
fn value_score(model: &ModelPricing) -> f64 {
    let cost = if model.unit == PricingUnit::Free {
        0.0
    } else {
        model.cost_per_unit
    };
    if cost <= 0.0 {
        return model.quality_score * FREE_MODEL_VALUE_MULTIPLIER;
    }
    model.quality_score / cost
}

fn matching_models(
    category: TaskCategory,
    pricing: &[ModelPricing],
) -> impl Iterator<Item = &ModelPricing> {
    pricing
        .iter()
        .filter(move |model| model.category == category)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{GpuType, ModelRecommendation, RecommendedModels};

    fn task(label: &str, category: TaskCategory, quantity: u32) -> TaskSpecification {
        TaskSpecification {
            label: label.into(),
            category,
            quantity,
        }
    }

    fn priced(
        model: &str,
        category: TaskCategory,
        cost_per_unit: f64,
        unit: PricingUnit,
        quality_score: f64,
    ) -> ModelPricing {
        ModelPricing {
            provider: if unit == PricingUnit::Free {
                LOCAL_PROVIDER.into()
            } else {
                "vendor".into()
            },
            model: model.into(),
            category,
            cost_per_unit,
            unit,
            quality_score,
            speed_score: 0.5,
            local_available: unit == PricingUnit::Free,
        }
    }

    fn slots(profile: &MachineProfile) -> [(TaskCategory, &ModelRecommendation); 7] {
        let recommended = &profile.recommended_models;
        [
            (TaskCategory::Text, &recommended.llm),
            (TaskCategory::Image, &recommended.image),
            (TaskCategory::Voice, &recommended.voice),
            (TaskCategory::Music, &recommended.music),
            (TaskCategory::Model3D, &recommended.model3d),
            (TaskCategory::Embedding, &recommended.embedding),
            (TaskCategory::Transcription, &recommended.transcription),
        ]
    }

    fn presets() -> Vec<MachineProfile> {
        MachineProfile::available_presets()
            .into_iter()
            .map(|(slug, _)| MachineProfile::from_preset(slug).expect("a listed preset"))
            .collect()
    }

    #[test]
    fn the_cheapest_image_model_is_a_free_local_one() {
        let pricing = default_pricing();
        let cheapest = CostEstimator::cheapest_for(TaskCategory::Image, &pricing).unwrap();
        assert_eq!(cheapest.model, "sdxl-comfyui");
        assert!(cheapest.local_available);
    }

    #[test]
    fn the_best_image_model_is_one_of_the_two_top_scorers() {
        let pricing = default_pricing();
        let best = CostEstimator::best_quality_for(TaskCategory::Image, &pricing).unwrap();
        assert!(best.quality_score >= 0.95);
        assert!(best.model == "flux-2-pro" || best.model == "gpt-image-1.5");
    }

    #[test]
    fn the_best_value_voice_model_is_the_cheap_paid_one() {
        let pricing = default_pricing();
        let best = CostEstimator::best_value_for(TaskCategory::Voice, &pricing).unwrap();
        assert_eq!(best.model, "fish-s1");
    }

    #[test]
    fn every_category_with_a_shipped_model_has_a_cheapest_model() {
        let pricing = default_pricing();
        for (category, expected) in [
            (TaskCategory::Text, "gemini-2.5-flash"),
            (TaskCategory::Image, "sdxl-comfyui"),
            (TaskCategory::Voice, "xtts-v2"),
            (TaskCategory::Music, "musicgen-large"),
            (TaskCategory::Model3D, "triposr"),
            (TaskCategory::Video, "kling-v2"),
        ] {
            let cheapest = CostEstimator::cheapest_for(category, &pricing).unwrap();
            assert_eq!(cheapest.model, expected, "{category:?}");
            assert_eq!(cheapest.category, category);
        }
        assert!(CostEstimator::cheapest_for(TaskCategory::Embedding, &pricing).is_none());
    }

    #[test]
    fn every_category_has_a_best_quality_model_of_its_own_kind() {
        let pricing = default_pricing();
        for (category, expected) in [
            (TaskCategory::Text, "claude-opus-5"),
            (TaskCategory::Voice, "eleven-v3"),
            (TaskCategory::Music, "suno-v5"),
            (TaskCategory::Model3D, "tripo-v2"),
            (TaskCategory::Video, "kling-v2"),
        ] {
            let best = CostEstimator::best_quality_for(category, &pricing).unwrap();
            assert_eq!(best.model, expected, "{category:?}");
        }
    }

    #[test]
    fn every_category_has_a_best_value_model_of_its_own_kind() {
        let pricing = default_pricing();
        for category in [
            TaskCategory::Text,
            TaskCategory::Image,
            TaskCategory::Model3D,
        ] {
            let best = CostEstimator::best_value_for(category, &pricing).unwrap();
            assert_eq!(best.category, category);
        }
    }

    #[test]
    fn local_first_picks_the_local_model_of_each_category() {
        let pricing = default_pricing();
        for (category, expected) in [
            (TaskCategory::Text, "llama-3.3-70b"),
            (TaskCategory::Image, "sdxl-comfyui"),
            (TaskCategory::Voice, "xtts-v2"),
            (TaskCategory::Music, "musicgen-large"),
            (TaskCategory::Model3D, "triposr"),
        ] {
            let chosen = CostEstimator::local_first_for(category, &pricing).unwrap();
            assert_eq!(chosen.model, expected, "{category:?}");
            assert!(chosen.local_available);
        }
    }

    #[test]
    fn local_first_never_offers_a_free_model_of_another_kind_for_video() {
        let pricing = default_pricing();
        let chosen = CostEstimator::local_first_for(TaskCategory::Video, &pricing).unwrap();
        assert_eq!(chosen.model, "kling-v2");
    }

    #[test]
    fn local_first_falls_back_to_best_value_when_nothing_runs_locally() {
        let pricing = vec![priced(
            "gpt-image-1.5",
            TaskCategory::Image,
            0.04,
            PricingUnit::PerImage,
            0.95,
        )];

        let chosen = CostEstimator::local_first_for(TaskCategory::Image, &pricing).unwrap();

        assert!(!chosen.local_available);
        assert_eq!(chosen.model, "gpt-image-1.5");
    }

    #[test]
    fn every_preset_offers_each_local_model_only_for_its_own_category() {
        for profile in presets() {
            let pricing = CostEstimator::with_hardware(&profile);

            for (category, recommendation) in slots(&profile) {
                let chosen = CostEstimator::local_first_for(category, &pricing);
                if recommendation.model_name == NO_LOCAL_MODEL {
                    assert!(
                        chosen.is_none_or(|model| !model.local_available),
                        "{} has no local {category:?} model but was offered {chosen:?}",
                        profile.name
                    );
                    continue;
                }
                let chosen = chosen.expect("a local model");
                assert_eq!(
                    chosen.model, recommendation.model_name,
                    "{} picked the wrong local {category:?} model",
                    profile.name
                );
                assert_eq!(chosen.category, category);
                assert!(chosen.local_available);
            }

            let video = CostEstimator::local_first_for(TaskCategory::Video, &pricing).unwrap();
            assert_eq!(video.model, "kling-v2", "{}", profile.name);
        }
    }

    #[test]
    fn every_preset_prices_each_modality_with_its_own_cheapest_model() {
        for profile in presets() {
            let pricing = CostEstimator::with_hardware(&profile);

            for (category, recommendation) in slots(&profile) {
                if category == TaskCategory::Text || recommendation.model_name == NO_LOCAL_MODEL {
                    continue;
                }
                let cheapest = CostEstimator::cheapest_for(category, &pricing).unwrap();
                assert_eq!(
                    cheapest.model, recommendation.model_name,
                    "{} priced {category:?} with the wrong model",
                    profile.name
                );
            }
        }
    }

    #[test]
    fn no_tasks_cost_nothing() {
        let pricing = default_pricing();
        let estimate = CostEstimator::estimate_batch_cost(&[], &pricing, CostStrategy::BestValue);
        assert_eq!(estimate.total_usd, 0.0);
        assert_eq!(estimate.quality_score, 0.0);
        assert!(estimate.breakdown.is_empty());
        assert!(estimate.unassigned.is_empty());
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
    fn a_token_price_is_charged_per_million_tokens() {
        let pricing = [priced(
            "claude-opus-5",
            TaskCategory::Text,
            25.0,
            PricingUnit::PerMillionTokens,
            0.98,
        )];
        let tasks = [task("Script", TaskCategory::Text, 200_000)];

        let estimate =
            CostEstimator::estimate_batch_cost(&tasks, &pricing, CostStrategy::BestQuality);

        assert_eq!(estimate.total_usd, 5.0);
        assert_eq!(estimate.breakdown[0].total_cost, 5.0);
        assert_eq!(estimate.breakdown[0].unit_cost, 0.000_025);
    }

    #[test]
    fn savings_count_only_the_tasks_that_have_a_local_option() {
        let pricing = [
            priced(
                "paid-image",
                TaskCategory::Image,
                0.04,
                PricingUnit::PerImage,
                0.95,
            ),
            priced(
                "local-image",
                TaskCategory::Image,
                0.0,
                PricingUnit::Free,
                0.8,
            ),
            priced(
                "paid-video",
                TaskCategory::Video,
                0.5,
                PricingUnit::PerVideoSecond,
                0.85,
            ),
        ];
        let tasks = [
            task("Images", TaskCategory::Image, 5),
            task("Video", TaskCategory::Video, 10),
        ];

        let estimate =
            CostEstimator::estimate_batch_cost(&tasks, &pricing, CostStrategy::BestQuality);

        assert_eq!(estimate.total_usd, 0.2 + 5.0);
        assert_eq!(estimate.local_savings_usd, 0.2);
    }

    #[test]
    fn a_task_no_model_can_do_is_reported_rather_than_dropped() {
        let pricing = default_pricing();
        let tasks = [
            task("Images", TaskCategory::Image, 1),
            task("Search index", TaskCategory::Embedding, 100),
        ];

        let estimate =
            CostEstimator::estimate_batch_cost(&tasks, &pricing, CostStrategy::BestValue);

        assert_eq!(estimate.breakdown.len(), 1);
        assert_eq!(estimate.unassigned, vec!["Search index".to_string()]);
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
        assert!(estimate.unassigned.is_empty());
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
        assert!((estimate.local_savings_usd - estimate.total_usd).abs() < 1e-12);
    }

    #[test]
    fn a_budget_that_covers_everything_changes_nothing() {
        let pricing = default_pricing();
        let tasks = [task("Image", TaskCategory::Image, 1)];

        let estimate =
            CostEstimator::estimate_batch_cost(&tasks, &pricing, CostStrategy::budget(100.0));

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

        let estimate =
            CostEstimator::estimate_batch_cost(&tasks, &pricing, CostStrategy::budget(3.0));

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

        let estimate =
            CostEstimator::estimate_batch_cost(&tasks, &pricing, CostStrategy::budget(0.01));

        assert!(estimate.total_usd <= 0.01 + f64::EPSILON);
        assert!(estimate.breakdown.iter().all(|item| item.is_local));
    }

    #[test]
    fn a_budget_reports_what_it_could_not_afford_and_recomputes_the_savings() {
        let pricing = [
            priced(
                "paid-image",
                TaskCategory::Image,
                1.0,
                PricingUnit::PerImage,
                0.95,
            ),
            priced(
                "local-image",
                TaskCategory::Image,
                0.0,
                PricingUnit::Free,
                0.8,
            ),
            priced(
                "paid-video",
                TaskCategory::Video,
                2.0,
                PricingUnit::PerVideoSecond,
                0.85,
            ),
        ];
        let tasks = [
            task("Images", TaskCategory::Image, 2),
            task("Video", TaskCategory::Video, 10),
        ];

        let estimate =
            CostEstimator::estimate_batch_cost(&tasks, &pricing, CostStrategy::budget(3.0));

        assert_eq!(estimate.total_usd, 2.0);
        assert_eq!(estimate.unassigned, vec!["Video".to_string()]);
        assert_eq!(estimate.local_savings_usd, 2.0);
        assert_eq!(estimate.breakdown.len(), 1);
        assert_eq!(estimate.breakdown[0].model, "paid-image");
    }

    #[test]
    fn a_hardware_profile_replaces_the_generic_local_entries() {
        let profile = MachineProfile::from_preset("m4-max-64").unwrap();
        let pricing = CostEstimator::with_hardware(&profile);

        let local: Vec<&ModelPricing> = pricing
            .iter()
            .filter(|entry| entry.local_available)
            .collect();
        assert_eq!(local.len(), 7);
        for entry in &local {
            assert_eq!(entry.cost_per_unit, 0.0);
            assert_eq!(entry.unit, PricingUnit::Free);
            assert!(entry.quality_score > 0.0);
        }

        let llm = local
            .iter()
            .find(|entry| entry.category == TaskCategory::Text)
            .unwrap();
        assert_eq!(llm.model, "qwen2.5:72b-instruct-q6_K");
        assert!((llm.quality_score - 0.92).abs() < f64::EPSILON);
    }

    #[test]
    fn a_smaller_machine_gets_a_lower_scoring_local_model() {
        let small =
            CostEstimator::with_hardware(&MachineProfile::from_preset("5900x-3080ti").unwrap());
        let large =
            CostEstimator::with_hardware(&MachineProfile::from_preset("m4-max-64").unwrap());

        let small_llm = CostEstimator::local_first_for(TaskCategory::Text, &small).unwrap();
        let large_llm = CostEstimator::local_first_for(TaskCategory::Text, &large).unwrap();

        assert!(
            small_llm.model.contains("qwen2.5:14b"),
            "{}",
            small_llm.model
        );
        assert!((small_llm.quality_score - 0.75).abs() < f64::EPSILON);
        assert!(large_llm.quality_score > small_llm.quality_score);
    }

    #[test]
    fn local_first_uses_the_hardware_entry_for_the_category_asked_for() {
        let profile = MachineProfile::from_preset("m4-max-64").unwrap();
        let pricing = CostEstimator::with_hardware(&profile);

        let chosen = CostEstimator::local_first_for(TaskCategory::Image, &pricing).unwrap();

        assert_eq!(chosen.model, profile.recommended_models.image.model_name);
        assert_eq!(chosen.cost_per_unit, 0.0);
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

    #[test]
    fn a_described_machine_offers_only_the_models_it_names() {
        let profile = MachineProfile::new(
            "Laptop",
            GpuType::CpuOnly,
            RecommendedModels::default()
                .with_llm(ModelRecommendation::new("qwen2.5:3b").with_quality_score(0.45)),
        );

        let pricing = CostEstimator::with_hardware(&profile);

        let local: Vec<(&str, TaskCategory)> = pricing
            .iter()
            .filter(|entry| entry.local_available)
            .map(|entry| (entry.model.as_str(), entry.category))
            .collect();

        assert_eq!(local, [("qwen2.5:3b", TaskCategory::Text)]);
    }
}
