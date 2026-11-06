//! Semantic category match against reference embeddings.

use crate::category_match::CategoryMatch;
use crate::similarity::cosine_similarity;

/// A named reference embedding used to classify an error or skip reason.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Category {
    /// Category name, such as `timeout` or `not_found`.
    pub name: String,
    /// Embedding of a description of that category.
    pub embedding: Vec<f32>,
}

impl Category {
    /// A category named `name` with `embedding`.
    pub fn new(name: impl Into<String>, embedding: Vec<f32>) -> Self {
        Self {
            name: name.into(),
            embedding,
        }
    }

    /// The closest category to `embedding` whose cosine score is at least
    /// `threshold`. `None` when nothing clears the floor, or when `embedding`
    /// is empty.
    pub fn nearest<'a>(
        embedding: &[f32],
        categories: &'a [Self],
        threshold: f64,
    ) -> Option<&'a Self> {
        if embedding.is_empty() {
            return None;
        }
        let mut best: Option<(&Category, f64)> = None;
        for category in categories {
            let score = cosine_similarity(embedding, &category.embedding);
            if score < threshold {
                continue;
            }
            match best {
                Some((_, best_score)) if score <= best_score => {}
                _ => best = Some((category, score)),
            }
        }
        best.map(|(category, _)| category)
    }

    /// The closest category to `embedding`, accepted only when it is both close
    /// and unambiguous.
    ///
    /// Entries sharing a [`name`](Self::name) are exemplars of one category,
    /// and a category scores as its closest exemplar. The winner must score at
    /// least `threshold` and lead the best other category by at least
    /// `minimum_margin`. That runner-up counts whether or not it clears
    /// `threshold` itself, and with a single category the margin is the
    /// winner's own score. Equal scores between categories rank by name, so the
    /// winning name, score and margin never depend on the order of
    /// `categories`. Among equally close exemplars of the winning category, the
    /// first one is returned.
    ///
    /// An exemplar whose length differs from `embedding`'s, or whose
    /// similarity is not finite, is skipped. `None` when `embedding` is empty,
    /// when every exemplar is skipped, or when the winner misses either bar.
    ///
    /// ```
    /// use abnegate_learn::Category;
    ///
    /// let categories = [
    ///     Category::new("timeout", vec![1.0, 0.0, 0.0]),
    ///     Category::new("timeout", vec![0.0, 1.0, 0.0]),
    ///     Category::new("network", vec![0.0, 0.0, 1.0]),
    /// ];
    ///
    /// let hit = Category::nearest_with_margin(&[0.1, 0.9, 0.1], &categories, 0.45, 0.05).unwrap();
    /// assert_eq!(hit.category.name, "timeout");
    /// assert!(hit.margin > 0.05);
    ///
    /// let between = Category::nearest_with_margin(&[0.5, 0.0, 0.5], &categories, 0.45, 0.05);
    /// assert!(between.is_none());
    /// ```
    pub fn nearest_with_margin<'a>(
        embedding: &[f32],
        categories: &'a [Self],
        threshold: f64,
        minimum_margin: f64,
    ) -> Option<CategoryMatch<'a>> {
        if embedding.is_empty() {
            return None;
        }
        let mut leader: Option<(&'a Self, f64)> = None;
        let mut runner_up: Option<(&'a Self, f64)> = None;
        for category in categories {
            if category.embedding.len() != embedding.len() {
                continue;
            }
            let score = cosine_similarity(embedding, &category.embedding);
            if !score.is_finite() {
                continue;
            }
            let candidate = (category, score);
            if shares_name(leader, category) {
                if outranks(candidate, leader) {
                    leader = Some(candidate);
                }
            } else if outranks(candidate, leader) {
                runner_up = leader;
                leader = Some(candidate);
            } else if outranks(candidate, runner_up) {
                runner_up = Some(candidate);
            }
        }
        let (category, score) = leader?;
        let margin = runner_up.map_or(score, |(_, runner_up_score)| score - runner_up_score);
        (score >= threshold && margin >= minimum_margin).then_some(CategoryMatch {
            category,
            score,
            margin,
        })
    }
}

fn shares_name(ranked: Option<(&Category, f64)>, category: &Category) -> bool {
    ranked.is_some_and(|(held, _)| held.name == category.name)
}

fn outranks(candidate: (&Category, f64), incumbent: Option<(&Category, f64)>) -> bool {
    let Some((held, held_score)) = incumbent else {
        return true;
    };
    let (category, score) = candidate;
    score > held_score || (score == held_score && category.name < held.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_picks_the_matching_reference() {
        let categories = [
            Category::new("timeout", vec![1.0, 0.0]),
            Category::new("syntax", vec![0.0, 1.0]),
        ];
        let hit = Category::nearest(&[0.95, 0.05], &categories, 0.3).unwrap();
        assert_eq!(hit.name, "timeout");
        assert!(Category::nearest(&[0.5, 0.5], &categories, 0.99).is_none());
    }

    const DIMENSION: usize = 6;
    const THRESHOLD: f64 = 0.45;
    const MARGIN: f64 = 0.05;
    const TOLERANCE: f64 = 1e-6;
    const NAMES: &[&str] = &["build", "dependency", "network", "test", "timeout"];

    fn basis(index: usize) -> Vec<f32> {
        let mut vector = vec![0.0; DIMENSION];
        vector[index] = 1.0;
        vector
    }

    fn blend(first: usize, second: usize, weight: f32) -> Vec<f32> {
        let mut vector = vec![0.0; DIMENSION];
        vector[first] = 1.0 - weight;
        vector[second] = weight;
        vector
    }

    fn references() -> Vec<Category> {
        NAMES
            .iter()
            .enumerate()
            .map(|(index, name)| Category::new(*name, basis(index)))
            .collect()
    }

    fn direction(name: &str) -> usize {
        NAMES
            .iter()
            .position(|candidate| *candidate == name)
            .expect("name is a reference")
    }

    fn named(found: Option<CategoryMatch<'_>>) -> Option<&str> {
        found.map(|found| found.category.name.as_str())
    }

    fn permutations(entries: &[Category]) -> Vec<Vec<Category>> {
        if entries.len() <= 1 {
            return vec![entries.to_vec()];
        }
        let mut orders = Vec::new();
        for index in 0..entries.len() {
            let mut rest = entries.to_vec();
            let head = rest.remove(index);
            for mut tail in permutations(&rest) {
                tail.insert(0, head.clone());
                orders.push(tail);
            }
        }
        orders
    }

    #[test]
    fn every_category_recovers_its_own_exemplar() {
        let categories = references();
        for name in NAMES {
            let found = Category::nearest_with_margin(
                &basis(direction(name)),
                &categories,
                THRESHOLD,
                MARGIN,
            )
            .expect("an exemplar matches itself");
            assert_eq!(found.category.name, *name);
            assert!((found.score - 1.0).abs() < TOLERANCE);
            assert!(
                (found.margin - 1.0).abs() < TOLERANCE,
                "orthogonal references leave the winner a full lead"
            );
        }
    }

    #[test]
    fn a_message_leaning_towards_another_category_keeps_its_own() {
        let categories = references();
        for weight in [0.0, 0.1, 0.2, 0.3] {
            let embedding = blend(direction("build"), direction("network"), weight);
            assert_eq!(
                named(Category::nearest_with_margin(
                    &embedding,
                    &categories,
                    THRESHOLD,
                    MARGIN
                )),
                Some("build"),
                "{weight} of the way towards network is still a build failure"
            );
        }
    }

    #[test]
    fn a_tie_within_the_margin_is_rejected() {
        let categories = references();
        let between = blend(direction("build"), direction("test"), 0.5);
        assert!(
            Category::nearest_with_margin(&between, &categories, THRESHOLD, MARGIN).is_none(),
            "an embedding equally like two categories must not be guessed at"
        );
    }

    #[test]
    fn a_lead_short_of_the_margin_is_rejected() {
        let categories = references();
        let close = blend(direction("build"), direction("test"), 0.49);
        let found = Category::nearest_with_margin(&close, &categories, THRESHOLD, 0.0)
            .expect("without a margin the leader wins");
        assert_eq!(found.category.name, "build");
        assert!(found.margin > 0.0 && found.margin < MARGIN);
        assert!(Category::nearest_with_margin(&close, &categories, THRESHOLD, MARGIN).is_none());
    }

    #[test]
    fn a_tie_is_admitted_by_name_once_the_margin_is_removed() {
        let between = blend(direction("build"), direction("test"), 0.5);
        for categories in permutations(&references()) {
            let found = Category::nearest_with_margin(&between, &categories, 0.4, 0.0)
                .expect("the margin is what rejects a tie");
            assert_eq!(found.category.name, "build", "equal scores rank by name");
            assert!(found.margin.abs() < TOLERANCE);
        }
    }

    #[test]
    fn a_score_below_the_threshold_is_rejected() {
        let categories = references();
        assert!(
            Category::nearest_with_margin(&basis(DIMENSION - 1), &categories, THRESHOLD, MARGIN)
                .is_none(),
            "an embedding resembling nothing must not be forced into the nearest category"
        );
        let weak = blend(direction("build"), DIMENSION - 1, 0.7);
        assert!(Category::nearest_with_margin(&weak, &categories, THRESHOLD, MARGIN).is_none());
        assert_eq!(
            named(Category::nearest_with_margin(
                &weak,
                &categories,
                0.3,
                MARGIN
            )),
            Some("build")
        );
    }

    #[test]
    fn a_runner_up_below_the_threshold_still_narrows_the_margin() {
        let categories = [
            Category::new("timeout", vec![1.0, 0.0]),
            Category::new("network", vec![0.0, 1.0]),
        ];
        let leaning = [0.9, 0.19_f32.sqrt()];
        let found = Category::nearest_with_margin(&leaning, &categories, 0.8, 0.4)
            .expect("a lead of about 0.46 clears 0.4");
        assert_eq!(found.category.name, "timeout");
        assert!((found.score - 0.9).abs() < TOLERANCE);
        assert!((found.margin - (0.9 - 0.19_f64.sqrt())).abs() < TOLERANCE);
        assert!(
            Category::nearest_with_margin(&leaning, &categories, 0.8, 0.5).is_none(),
            "network scores under the threshold yet still stands as runner-up"
        );
    }

    #[test]
    fn an_empty_embedding_matches_nothing() {
        assert!(Category::nearest_with_margin(&[], &references(), THRESHOLD, MARGIN).is_none());
        assert!(Category::nearest_with_margin(&[], &references(), -1.0, -2.0).is_none());
    }

    #[test]
    fn no_categories_match_nothing() {
        assert!(Category::nearest_with_margin(&basis(0), &[], THRESHOLD, MARGIN).is_none());
    }

    #[test]
    fn a_single_category_leads_by_its_own_score() {
        let categories = [Category::new("timeout", vec![1.0, 0.0])];
        let found = Category::nearest_with_margin(&[0.8, 0.6], &categories, THRESHOLD, MARGIN)
            .expect("an uncontested category clears both bars");
        assert_eq!(found.category.name, "timeout");
        assert!((found.score - 0.8).abs() < TOLERANCE);
        assert!((found.margin - found.score).abs() < TOLERANCE);
    }

    #[test]
    fn a_single_category_must_still_clear_the_margin_with_its_score() {
        let categories = [Category::new("timeout", vec![1.0, 0.0])];
        let faint = [0.03, 1.0];
        assert!(Category::nearest_with_margin(&faint, &categories, 0.0, MARGIN).is_none());
        assert_eq!(
            named(Category::nearest_with_margin(&faint, &categories, 0.0, 0.0)),
            Some("timeout")
        );
    }

    #[test]
    fn an_embedding_from_another_model_matches_nothing() {
        assert!(
            Category::nearest_with_margin(&[1.0, 0.0, 0.0], &references(), THRESHOLD, MARGIN)
                .is_none(),
            "a vector of another dimension must not be scored against these references"
        );
    }

    #[test]
    fn an_exemplar_of_another_dimension_is_skipped() {
        let categories = [
            Category::new("timeout", vec![1.0, 0.0]),
            Category::new("network", vec![1.0, 0.0, 0.0]),
        ];
        let found = Category::nearest_with_margin(&[-1.0, 0.0], &categories, -1.0, -1.0)
            .expect("timeout is the only scorable category");
        assert_eq!(
            found.category.name, "timeout",
            "an exemplar from another model must not win on its default score"
        );
        assert!((found.score + 1.0).abs() < TOLERANCE);
        assert!(
            (found.margin + 1.0).abs() < TOLERANCE,
            "an exemplar from another model must not stand as runner-up"
        );
    }

    #[test]
    fn a_category_scores_as_its_closest_exemplar() {
        let categories = [
            Category::new("build", basis(0)),
            Category::new("test", basis(1)),
            Category::new("build", basis(2)),
        ];
        let found = Category::nearest_with_margin(&basis(2), &categories, THRESHOLD, MARGIN)
            .expect("the second build exemplar matches exactly");
        assert_eq!(found.category.name, "build");
        assert!(std::ptr::eq(found.category, &categories[2]));
        assert!((found.score - 1.0).abs() < TOLERANCE);
        assert!((found.margin - 1.0).abs() < TOLERANCE);
    }

    #[test]
    fn exemplars_of_one_category_never_compete_with_each_other() {
        let categories = [
            Category::new("build", blend(0, 1, 0.01)),
            Category::new("build", blend(0, 1, 0.02)),
            Category::new("test", basis(3)),
        ];
        let found = Category::nearest_with_margin(&basis(0), &categories, THRESHOLD, MARGIN)
            .expect("near-identical exemplars of one category are not a tie");
        assert_eq!(found.category.name, "build");
        assert!(
            found.margin > 0.9,
            "the margin is measured against test, not build"
        );
    }

    #[test]
    fn the_answer_does_not_depend_on_order() {
        let categories = [
            Category::new("build", basis(0)),
            Category::new("test", basis(1)),
            Category::new("build", blend(1, 2, 0.9)),
            Category::new("network", blend(1, 2, 0.6)),
            Category::new("test", basis(2)),
        ];
        let embedding = blend(1, 2, 0.8);
        let expected = Category::nearest_with_margin(&embedding, &categories, 0.0, 0.0)
            .map(|found| (found.category.name.clone(), found.score, found.margin));
        assert!(expected.is_some());
        for order in permutations(&categories) {
            let found = Category::nearest_with_margin(&embedding, &order, 0.0, 0.0)
                .map(|found| (found.category.name.clone(), found.score, found.margin));
            assert_eq!(found, expected, "order {order:?} changed the answer");
        }
    }

    #[test]
    fn a_non_finite_exemplar_is_ignored() {
        let categories = [
            Category::new("timeout", vec![1.0, 0.0]),
            Category::new("network", vec![f32::NAN, 1.0]),
        ];
        let found = Category::nearest_with_margin(&[1.0, 0.0], &categories, THRESHOLD, MARGIN)
            .expect("an unscorable exemplar is not a runner-up");
        assert_eq!(found.category.name, "timeout");
        assert!((found.margin - 1.0).abs() < TOLERANCE);
        assert!(
            Category::nearest_with_margin(&[f32::NAN, 0.0], &categories, -1.0, -2.0).is_none(),
            "an unscorable embedding matches nothing"
        );
    }

    #[test]
    fn a_nan_bar_rejects_every_match() {
        let categories = references();
        assert!(Category::nearest_with_margin(&basis(0), &categories, f64::NAN, MARGIN).is_none());
        assert!(
            Category::nearest_with_margin(&basis(0), &categories, THRESHOLD, f64::NAN).is_none()
        );
    }
}
