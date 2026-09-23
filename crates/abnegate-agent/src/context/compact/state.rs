use serde::Deserialize;
use serde::Serialize;

/// The structured record a summarizer returns, and the only shape it may.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct State {
    objective: String,
    constraints: Vec<String>,
    corrections: Vec<String>,
    decisions: Vec<String>,
    completed: Vec<String>,
    evidence: Vec<String>,
    failed: Vec<String>,
    pending: Vec<String>,
    questions: Vec<String>,
}

impl State {
    pub(super) fn meaningful(&self) -> bool {
        !self.objective.trim().is_empty()
            || [
                &self.constraints,
                &self.corrections,
                &self.decisions,
                &self.completed,
                &self.evidence,
                &self.failed,
                &self.pending,
                &self.questions,
            ]
            .into_iter()
            .flatten()
            .any(|item| !item.trim().is_empty())
    }
}
