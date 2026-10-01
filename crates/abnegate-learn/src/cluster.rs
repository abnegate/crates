//! Repeated (scope, strategy, verdict, error) groups.

use crate::trial::Trial;
use crate::verdict::Verdict;

/// A group of trials that failed the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Cluster {
    /// Stable key for the group.
    pub key: String,
    /// Scope the trials share.
    pub scope: String,
    /// Strategy the trials share.
    pub strategy: String,
    /// Verdict the trials share.
    pub verdict: Verdict,
    /// Error or skip reason the trials share, empty when none was recorded.
    pub error: String,
    /// Trial ids in the group, oldest first.
    pub trial_ids: Vec<i64>,
}

impl Cluster {
    /// Group `trials` by scope, strategy, verdict, and error.
    ///
    /// Clusters with a single trial are kept so a host can still stamp "this
    /// already failed once". Larger groups sort first.
    pub fn group(trials: &[Trial]) -> Vec<Self> {
        let mut order: Vec<String> = Vec::new();
        let mut groups: std::collections::BTreeMap<String, Cluster> =
            std::collections::BTreeMap::new();

        for trial in trials {
            let error = trial.error.clone().unwrap_or_default();
            let key = format!(
                "{}:{}:{}:{}",
                trial.scope,
                trial.strategy,
                trial.verdict.as_str(),
                error
            );
            if let Some(cluster) = groups.get_mut(&key) {
                cluster.trial_ids.push(trial.id);
                continue;
            }
            order.push(key.clone());
            groups.insert(
                key.clone(),
                Self {
                    key,
                    scope: trial.scope.clone(),
                    strategy: trial.strategy.clone(),
                    verdict: trial.verdict,
                    error,
                    trial_ids: vec![trial.id],
                },
            );
        }

        let mut clusters: Vec<Cluster> = order
            .into_iter()
            .filter_map(|key| groups.remove(&key))
            .collect();
        clusters.sort_by(|left, right| {
            right
                .trial_ids
                .len()
                .cmp(&left.trial_ids.len())
                .then_with(|| left.key.cmp(&right.key))
        });
        clusters
    }

    /// How many trials are in the group.
    pub fn count(&self) -> usize {
        self.trial_ids.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Memory;
    use crate::trial_input::TrialInput;

    #[test]
    fn repeated_skips_collapse_into_one_cluster() {
        let mut memory = Memory::new();
        for _ in 0..3 {
            memory.record(
                TrialInput::new("webkit", "webkit.fuzz")
                    .with_verdict(Verdict::Skip)
                    .with_error("reprl-unavailable"),
            );
        }
        memory.record(TrialInput::new("webkit", "webkit.jit").with_verdict(Verdict::Empty));
        let clusters = Cluster::group(memory.all());
        assert_eq!(clusters[0].strategy, "webkit.fuzz");
        assert_eq!(clusters[0].count(), 3);
        assert_eq!(clusters[0].error, "reprl-unavailable");
        assert_eq!(clusters[1].strategy, "webkit.jit");
        assert_eq!(clusters[1].count(), 1);
    }
}
