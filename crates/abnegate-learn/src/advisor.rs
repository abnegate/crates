//! Suggestions drawn from similar trials.

use crate::config::Config;
use crate::kind::SuggestionKind;
use crate::similar::SimilarTrial;
use crate::suggestion::Suggestion;

/// Turns similar trials into avoid/context/instruction/warning suggestions.
#[derive(Debug, Clone, PartialEq)]
pub struct Advisor {
    config: Config,
}

impl Default for Advisor {
    fn default() -> Self {
        Self::new()
    }
}

impl Advisor {
    /// An advisor with the default similarity limits.
    pub fn new() -> Self {
        Self {
            config: Config::new(),
        }
    }

    /// An advisor with `config`.
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// Suggestions from `similar`, highest confidence first, cut to
    /// [`Config::suggestion_limit`](crate::Config::suggestion_limit).
    pub fn suggestions(&self, similar: &[SimilarTrial]) -> Vec<Suggestion> {
        let mut suggestions = Vec::new();
        let successful: Vec<&SimilarTrial> = similar
            .iter()
            .filter(|item| item.trial.verdict.is_positive())
            .collect();
        let failed: Vec<&SimilarTrial> = similar
            .iter()
            .filter(|item| item.trial.verdict.is_negative())
            .collect();

        for item in &successful {
            if let Some(lesson) = &item.trial.lesson {
                suggestions.push(Suggestion::new(
                    SuggestionKind::Context,
                    format!("{} succeeded. Learning: {lesson}", item.trial.strategy),
                    item.score,
                    vec![item.trial.id],
                ));
                suggestions.push(Suggestion::new(
                    SuggestionKind::Instruction,
                    format!("Apply: {lesson}"),
                    item.score,
                    vec![item.trial.id],
                ));
            }
        }

        if successful.len() >= 2 {
            let ids: Vec<i64> = successful.iter().map(|item| item.trial.id).collect();
            let confidence = mean(successful.iter().map(|item| item.score));
            suggestions.push(Suggestion::new(
                SuggestionKind::Context,
                format!("{} similar attempts succeeded before.", successful.len()),
                confidence,
                ids,
            ));
        }

        for item in &failed {
            let text = if let Some(lesson) = &item.trial.lesson {
                format!("{} already failed: {lesson}", item.trial.strategy)
            } else if let Some(error) = &item.trial.error {
                format!(
                    "{} already {} with {error}",
                    item.trial.strategy, item.trial.verdict
                )
            } else {
                format!(
                    "{} already tried ({})",
                    item.trial.strategy, item.trial.verdict
                )
            };
            suggestions.push(Suggestion::new(
                SuggestionKind::Avoid,
                text,
                item.score,
                vec![item.trial.id],
            ));
        }

        let errors: Vec<&str> = failed
            .iter()
            .filter_map(|item| item.trial.error.as_deref())
            .collect();
        if !errors.is_empty() {
            let ids: Vec<i64> = failed.iter().map(|item| item.trial.id).collect();
            suggestions.push(Suggestion::new(
                SuggestionKind::Warning,
                format!("Similar attempts failed with: {}.", unique_join(&errors)),
                mean(failed.iter().map(|item| item.score)),
                ids,
            ));
        }

        if failed.len() > successful.len() && similar.len() >= 3 {
            suggestions.push(Suggestion::new(
                SuggestionKind::Warning,
                format!(
                    "{} of {} similar attempts failed.",
                    failed.len(),
                    similar.len()
                ),
                0.7,
                failed.iter().map(|item| item.trial.id).collect(),
            ));
        }

        suggestions.sort_by(|left, right| {
            right
                .confidence
                .partial_cmp(&left.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        suggestions.truncate(self.config.suggestion_limit);
        suggestions
    }
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let values: Vec<f64> = values.collect();
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn unique_join(values: &[&str]) -> String {
    let mut seen = std::collections::BTreeSet::new();
    let mut unique = Vec::new();
    for value in values {
        if seen.insert(*value) {
            unique.push(*value);
        }
    }
    unique.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Memory;
    use crate::trial_input::TrialInput;
    use crate::verdict::Verdict;

    #[test]
    fn failed_neighbours_become_avoid_suggestions() {
        let mut memory = Memory::new();
        memory.record(
            TrialInput::new("webkit", "webkit.fuzz")
                .with_verdict(Verdict::Skip)
                .with_error("reprl-unavailable")
                .with_lesson("fuzz cannot run without reprl")
                .with_embedding(vec![1.0, 0.0]),
        );
        let similar = memory.similar(&[1.0, 0.0]);
        let suggestions = Advisor::new().suggestions(&similar);
        assert!(
            suggestions
                .iter()
                .any(|item| item.kind == SuggestionKind::Avoid
                    && item.text.contains("webkit.fuzz"))
        );
    }

    #[test]
    fn successful_lessons_become_context_and_instruction() {
        let mut memory = Memory::new();
        memory.record(
            TrialInput::new("src", "issue-1")
                .with_verdict(Verdict::Success)
                .with_lesson("check token expiration")
                .with_embedding(vec![1.0, 0.0]),
        );
        let suggestions = Advisor::new().suggestions(&memory.similar(&[1.0, 0.0]));
        assert!(
            suggestions
                .iter()
                .any(|item| item.kind == SuggestionKind::Context
                    && item.text.contains("token expiration"))
        );
        assert!(
            suggestions
                .iter()
                .any(|item| item.kind == SuggestionKind::Instruction
                    && item.text.contains("Apply: check token expiration"))
        );
    }
}
