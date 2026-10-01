//! In-memory trial store.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::advisor::Advisor;
use crate::cluster::Cluster;
use crate::config::Config;
use crate::digest::Digest;
use crate::similar::SimilarTrial;
use crate::similarity::cosine_similarity;
use crate::suggestion::Suggestion;
use crate::trial::Trial;
use crate::trial_input::TrialInput;

/// In-memory trial store.
///
/// Record each attempt, hydrate from a host database with [`load`](Self::load),
/// and ask [`digest`](Self::digest) for the scoped picture of what already
/// failed. Similarity uses embeddings the host attaches; a trial with no
/// embedding still contributes to clusters and the failed-strategy list.
#[derive(Debug, Clone)]
pub struct Memory {
    trials: Vec<Trial>,
    next_id: i64,
    config: Config,
}

impl Default for Memory {
    fn default() -> Self {
        Self::new()
    }
}

impl Memory {
    /// An empty memory with the default similarity limits.
    pub fn new() -> Self {
        Self {
            trials: Vec::new(),
            next_id: 1,
            config: Config::new(),
        }
    }

    /// This memory with `config`.
    pub fn with_config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    /// Limits used for similarity search and suggestions.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Append previously recorded trials, such as rows loaded from a database.
    ///
    /// The next [`record`](Self::record) id is one past the highest loaded id.
    pub fn load(&mut self, trials: impl IntoIterator<Item = Trial>) {
        self.trials.extend(trials);
        if let Some(highest) = self.trials.iter().map(|trial| trial.id).max() {
            self.next_id = highest.max(0) + 1;
        }
    }

    /// Record `input` and return its id.
    ///
    /// [`TrialInput::with_id`] keeps a host-assigned id. Otherwise the next
    /// id in this memory is used. A zero `recorded_at` is replaced with the
    /// current unix seconds.
    pub fn record(&mut self, input: TrialInput) -> i64 {
        let id = match input.id {
            Some(id) if id > 0 => {
                if id >= self.next_id {
                    self.next_id = id + 1;
                }
                id
            }
            _ => {
                let id = self.next_id;
                self.next_id += 1;
                id
            }
        };
        let recorded_at = if input.recorded_at == 0 {
            unix_seconds()
        } else {
            input.recorded_at
        };
        self.trials.push(Trial::from_parts(
            id,
            input.scope,
            input.strategy,
            input.action,
            input.verdict,
            input.summary,
            input.error,
            input.lesson,
            input.embedding,
            recorded_at,
        ));
        id
    }

    /// Attach `embedding` to the trial `id`, if it exists.
    pub fn attach_embedding(&mut self, id: i64, embedding: Vec<f32>) {
        if embedding.is_empty() {
            return;
        }
        if let Some(trial) = self.trials.iter_mut().find(|trial| trial.id == id) {
            trial.embedding = Some(embedding);
        }
    }

    /// Attach `lesson` to the trial `id`, if it exists.
    pub fn attach_lesson(&mut self, id: i64, lesson: impl Into<String>) {
        let lesson = lesson.into();
        if lesson.is_empty() {
            return;
        }
        if let Some(trial) = self.trials.iter_mut().find(|trial| trial.id == id) {
            trial.lesson = Some(lesson);
        }
    }

    /// The trial `id`, when it exists.
    pub fn get(&self, id: i64) -> Option<&Trial> {
        self.trials.iter().find(|trial| trial.id == id)
    }

    /// Every recorded trial, oldest first.
    pub fn all(&self) -> &[Trial] {
        &self.trials
    }

    /// Trials in `scope`, oldest first.
    pub fn in_scope(&self, scope: &str) -> Vec<&Trial> {
        self.trials
            .iter()
            .filter(|trial| trial.scope == scope)
            .collect()
    }

    /// Trials with a negative verdict, optionally restricted to `scope`.
    pub fn failed(&self, scope: Option<&str>) -> Vec<&Trial> {
        self.trials
            .iter()
            .filter(|trial| trial.verdict.is_negative())
            .filter(|trial| scope.is_none_or(|scope| trial.scope == scope))
            .collect()
    }

    /// Distinct negative strategies in `scope`, most recent first.
    pub fn failed_strategies(&self, scope: &str) -> Vec<String> {
        let mut names = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for trial in self.trials.iter().rev() {
            if trial.scope != scope || !trial.verdict.is_negative() {
                continue;
            }
            if seen.insert(trial.strategy.clone()) {
                names.push(trial.strategy.clone());
            }
        }
        names
    }

    /// Neighbours of `embedding` at or above the configured similarity floor.
    pub fn similar(&self, embedding: &[f32]) -> Vec<SimilarTrial> {
        self.similar_filtered(embedding, |_| true)
    }

    /// Neighbours of `embedding` restricted to `scope`.
    pub fn similar_in_scope(&self, scope: &str, embedding: &[f32]) -> Vec<SimilarTrial> {
        self.similar_filtered(embedding, |trial| trial.scope == scope)
    }

    /// Suggestions from neighbours of `embedding`.
    pub fn suggestions(&self, embedding: &[f32]) -> Vec<Suggestion> {
        Advisor::with_config(self.config.clone()).suggestions(&self.similar(embedding))
    }

    /// Suggestions from neighbours of `embedding` restricted to `scope`.
    pub fn suggestions_in_scope(&self, scope: &str, embedding: &[f32]) -> Vec<Suggestion> {
        Advisor::with_config(self.config.clone())
            .suggestions(&self.similar_in_scope(scope, embedding))
    }

    /// Prompt with a learnings block prepended. `base` is unchanged when there
    /// are no suggestions.
    pub fn enhance(&self, base: &str, embedding: &[f32]) -> String {
        enhance(base, &self.suggestions(embedding))
    }

    /// Prompt with a learnings block prepended, restricted to `scope`.
    pub fn enhance_in_scope(&self, scope: &str, base: &str, embedding: &[f32]) -> String {
        enhance(base, &self.suggestions_in_scope(scope, embedding))
    }

    /// Failure clusters, optionally restricted to `scope`.
    pub fn clusters(&self, scope: Option<&str>) -> Vec<Cluster> {
        let owned: Vec<Trial> = match scope {
            Some(scope) => self
                .trials
                .iter()
                .filter(|trial| trial.scope == scope)
                .cloned()
                .collect(),
            None => self.trials.clone(),
        };
        Cluster::group(&owned)
    }

    /// How often trials in `scope` (or all trials) are positive.
    pub fn success_rate(&self, scope: Option<&str>) -> f64 {
        let filtered: Vec<&Trial> = self
            .trials
            .iter()
            .filter(|trial| scope.is_none_or(|scope| trial.scope == scope))
            .collect();
        if filtered.is_empty() {
            return 0.0;
        }
        let successes = filtered
            .iter()
            .filter(|trial| trial.verdict.is_positive())
            .count();
        successes as f64 / filtered.len() as f64
    }

    /// Error strings and how often they appear, highest count first.
    pub fn common_errors(&self, scope: Option<&str>, limit: usize) -> Vec<(String, usize)> {
        let mut counts = std::collections::BTreeMap::<String, usize>::new();
        for trial in &self.trials {
            if let Some(scope) = scope
                && trial.scope != scope
            {
                continue;
            }
            if let Some(error) = &trial.error {
                *counts.entry(error.clone()).or_insert(0) += 1;
            }
        }
        let mut items: Vec<(String, usize)> = counts.into_iter().collect();
        items.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        items.truncate(limit);
        items
    }

    /// Scoped picture of what already failed, for the next round.
    ///
    /// `embedding` is the query for similar-trial suggestions. Without one,
    /// the digest still includes clusters and failed strategies.
    pub fn digest(&self, scope: &str, embedding: Option<&[f32]>) -> Digest {
        let suggestions = match embedding {
            Some(embedding) if !embedding.is_empty() => self.suggestions_in_scope(scope, embedding),
            _ => Vec::new(),
        };
        Digest {
            suggestions,
            clusters: self.clusters(Some(scope)),
            failed_strategies: self.failed_strategies(scope),
        }
    }

    fn similar_filtered(
        &self,
        embedding: &[f32],
        keep: impl Fn(&Trial) -> bool,
    ) -> Vec<SimilarTrial> {
        if embedding.is_empty() || self.config.similar_limit == 0 {
            return Vec::new();
        }
        let mut hits: Vec<SimilarTrial> = self
            .trials
            .iter()
            .filter(|trial| keep(trial))
            .filter_map(|trial| {
                let score = cosine_similarity(embedding, trial.embedding.as_deref()?);
                if score >= self.config.minimum_similarity {
                    Some(SimilarTrial::new(trial.clone(), score))
                } else {
                    None
                }
            })
            .collect();
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(self.config.similar_limit);
        hits
    }
}

fn enhance(base: &str, suggestions: &[Suggestion]) -> String {
    if suggestions.is_empty() {
        return base.to_string();
    }
    let mut lines = Vec::from([
        "# Learnings from similar attempts".to_string(),
        String::new(),
    ]);
    for suggestion in suggestions {
        lines.push(format!("- {}: {}", suggestion.kind, suggestion.text));
    }
    lines.push(String::new());
    lines.push("---".into());
    lines.push(String::new());
    lines.push(base.to_string());
    lines.join("\n")
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verdict::Verdict;

    #[test]
    fn a_later_round_in_the_same_scope_sees_every_failed_trial() {
        let mut memory = Memory::new();
        memory.record(
            TrialInput::new("webkit", "webkit.fuzz")
                .with_verdict(Verdict::Skip)
                .with_error("reprl-unavailable")
                .with_embedding(vec![1.0, 0.0, 0.0]),
        );
        memory.record(
            TrialInput::new("webkit", "webkit.sanitizer")
                .with_verdict(Verdict::Empty)
                .with_embedding(vec![0.9, 0.1, 0.0]),
        );
        memory.record(
            TrialInput::new("kernel", "kernel.campaign")
                .with_verdict(Verdict::Skip)
                .with_error("campaign=blocked")
                .with_embedding(vec![0.0, 1.0, 0.0]),
        );

        let webkit = memory.digest("webkit", Some(&[1.0, 0.0, 0.0]));
        assert_eq!(
            webkit.failed_strategies,
            vec!["webkit.sanitizer".to_string(), "webkit.fuzz".to_string()]
        );
        assert!(
            webkit
                .suggestions
                .iter()
                .any(|item| item.text.contains("webkit.fuzz"))
        );
        assert!(!webkit.as_prompt().contains("kernel.campaign"));

        let kernel = memory.digest("kernel", None);
        assert_eq!(
            kernel.failed_strategies,
            vec!["kernel.campaign".to_string()]
        );
        assert!(!kernel.as_prompt().contains("webkit.fuzz"));
    }

    #[test]
    fn load_keeps_host_ids_and_the_next_record_does_not_collide() {
        let mut memory = Memory::new();
        memory.load([Trial::from_parts(
            40,
            "lab".into(),
            "fuzz".into(),
            String::new(),
            Verdict::Skip,
            String::new(),
            Some("missing".into()),
            None,
            None,
            1,
        )]);
        let id = memory.record(TrialInput::new("lab", "jit").with_verdict(Verdict::Empty));
        assert_eq!(id, 41);
        assert_eq!(memory.get(40).unwrap().strategy, "fuzz");
    }

    #[test]
    fn enhance_leaves_the_base_prompt_when_nothing_is_similar() {
        let memory = Memory::new();
        assert_eq!(memory.enhance("try fuzz", &[1.0, 0.0]), "try fuzz");
    }
}
