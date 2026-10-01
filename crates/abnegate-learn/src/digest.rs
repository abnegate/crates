//! Scoped picture of what already failed.

use crate::cluster::Cluster;
use crate::kind::SuggestionKind;
use crate::suggestion::Suggestion;

/// Scoped picture of what already failed, for the next round.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct Digest {
    /// Suggestions drawn from similar trials.
    pub suggestions: Vec<Suggestion>,
    /// Repeated failure groups in this scope.
    pub clusters: Vec<Cluster>,
    /// Distinct strategies that have a negative verdict, most recent first.
    pub failed_strategies: Vec<String>,
}

impl Digest {
    /// An empty digest.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether there is nothing to stamp onto the next attempt.
    pub fn is_empty(&self) -> bool {
        self.suggestions.is_empty() && self.clusters.is_empty() && self.failed_strategies.is_empty()
    }

    /// `(key, value)` rows a host can stamp onto the next attempt.
    ///
    /// Keys use a `learn_` prefix so a prior-stage copier that prefixes
    /// `prior_` produces `prior_learn_*`.
    pub fn entries(&self) -> Vec<(String, String)> {
        let mut entries = Vec::new();
        if !self.failed_strategies.is_empty() {
            entries.push(("learn_failed".into(), self.failed_strategies.join(",")));
        }
        for (index, cluster) in self.clusters.iter().enumerate() {
            entries.push((
                format!("learn_cluster_{index}_strategy"),
                cluster.strategy.clone(),
            ));
            entries.push((
                format!("learn_cluster_{index}_verdict"),
                cluster.verdict.as_str().into(),
            ));
            entries.push((
                format!("learn_cluster_{index}_count"),
                cluster.count().to_string(),
            ));
            if !cluster.error.is_empty() {
                entries.push((
                    format!("learn_cluster_{index}_error"),
                    cluster.error.clone(),
                ));
            }
        }
        let mut counts = std::collections::BTreeMap::<SuggestionKind, usize>::new();
        for suggestion in &self.suggestions {
            let index = counts.entry(suggestion.kind).or_insert(0);
            entries.push((
                format!("learn_{}_{index}", suggestion.kind.as_str()),
                suggestion.text.clone(),
            ));
            *index += 1;
        }
        entries
    }

    /// Prompt block a model can read. Empty when the digest is empty.
    pub fn as_prompt(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut lines = Vec::from(["# Learnings".to_string(), String::new()]);
        if !self.failed_strategies.is_empty() {
            lines.push(format!(
                "Failed strategies: {}.",
                self.failed_strategies.join(", ")
            ));
        }
        for cluster in &self.clusters {
            let mut line = format!(
                "- {} {} {} time{}",
                cluster.strategy,
                cluster.verdict,
                cluster.count(),
                if cluster.count() == 1 { "" } else { "s" }
            );
            if !cluster.error.is_empty() {
                line.push_str(" (");
                line.push_str(&cluster.error);
                line.push(')');
            }
            line.push('.');
            lines.push(line);
        }
        for suggestion in &self.suggestions {
            let prefix = match suggestion.kind {
                SuggestionKind::Context => "Context",
                SuggestionKind::Avoid => "Avoid",
                SuggestionKind::Instruction => "Instruction",
                SuggestionKind::Warning => "Warning",
            };
            lines.push(format!("- {prefix}: {}", suggestion.text));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use crate::Memory;
    use crate::trial_input::TrialInput;
    use crate::verdict::Verdict;

    #[test]
    fn entries_use_the_learn_prefix() {
        let mut memory = Memory::new();
        memory.record(
            TrialInput::new("webkit", "webkit.fuzz")
                .with_verdict(Verdict::Skip)
                .with_error("reprl-unavailable"),
        );
        let entries = memory.digest("webkit", None).entries();
        assert!(
            entries
                .iter()
                .any(|(key, value)| key == "learn_failed" && value.contains("webkit.fuzz"))
        );
        assert!(
            entries
                .iter()
                .any(|(key, _)| key == "learn_cluster_0_strategy")
        );
    }
}
