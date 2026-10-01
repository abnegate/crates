//! One recorded attempt.

use crate::trial_input::TrialInput;
use crate::verdict::Verdict;

/// One recorded attempt.
///
/// Built by [`Memory::record`](crate::Memory::record) from a
/// [`TrialInput`](crate::TrialInput), or reconstructed with
/// [`from_input`](Self::from_input) when hydrating from a host store.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Trial {
    /// Identifier assigned by [`Memory`](crate::Memory), or the host's own id
    /// when hydrating with [`TrialInput::with_id`](crate::TrialInput::with_id).
    pub id: i64,
    /// Caller-defined namespace, such as a lab or repository.
    pub scope: String,
    /// Named approach that was tried.
    pub strategy: String,
    /// Action taken under that strategy, empty when the strategy is the action.
    pub action: String,
    /// How it turned out.
    pub verdict: Verdict,
    /// Short description of what happened.
    pub summary: String,
    /// Error or skip reason, when there was one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Extracted lesson the next round should read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lesson: Option<String>,
    /// Optional embedding of the trial text, used for similarity search.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    /// Keyword tags extracted from the trial text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Unix seconds when the trial was recorded. Zero means unset.
    pub recorded_at: i64,
}

impl Trial {
    /// Reconstruct a trial from a host-supplied [`TrialInput`].
    ///
    /// [`TrialInput::with_id`] sets `id`; otherwise it is `0` until
    /// [`Memory::record`](crate::Memory::record) assigns one.
    pub fn from_input(input: TrialInput) -> Self {
        Self {
            id: input.id.unwrap_or(0),
            scope: input.scope,
            strategy: input.strategy,
            action: input.action,
            verdict: input.verdict,
            summary: input.summary,
            error: input.error,
            lesson: input.lesson,
            embedding: input.embedding,
            tags: input.tags,
            recorded_at: input.recorded_at,
        }
    }

    /// Text used when a host embeds the trial itself.
    pub fn embed_text(&self) -> String {
        let mut parts = Vec::new();
        if !self.strategy.is_empty() {
            parts.push(self.strategy.as_str());
        }
        if !self.action.is_empty() && self.action != self.strategy {
            parts.push(self.action.as_str());
        }
        parts.push(self.verdict.as_str());
        if !self.summary.is_empty() {
            parts.push(self.summary.as_str());
        }
        if let Some(error) = &self.error {
            parts.push(error.as_str());
        }
        if let Some(lesson) = &self.lesson {
            parts.push(lesson.as_str());
        }
        for tag in &self.tags {
            parts.push(tag.as_str());
        }
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trial_input::TrialInput;

    #[test]
    fn from_input_keeps_a_host_id_and_tags() {
        let trial = Trial::from_input(
            TrialInput::new("lab", "fuzz")
                .with_id(40)
                .with_verdict(Verdict::Skip)
                .with_tags(vec!["reprl".into()]),
        );
        assert_eq!(trial.id, 40);
        assert_eq!(trial.tags, vec!["reprl".to_string()]);
        assert!(trial.embed_text().contains("reprl"));
    }

    #[test]
    fn serde_round_trips_a_trial() {
        let trial = Trial::from_input(
            TrialInput::new("lab", "fuzz")
                .with_id(1)
                .with_verdict(Verdict::Skip)
                .with_error("reprl-unavailable")
                .with_embedding(vec![1.0, 0.0]),
        );
        let json = serde_json::to_string(&trial).unwrap();
        let parsed: Trial = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, trial);
    }
}
