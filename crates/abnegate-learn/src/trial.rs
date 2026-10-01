//! One recorded attempt.

use crate::verdict::Verdict;

/// One recorded attempt.
///
/// Built by [`Memory::record`](crate::Memory::record) from a
/// [`TrialInput`](crate::TrialInput). Public fields are for reading; a caller
/// that supplies a trial uses the input builder.
#[derive(Debug, Clone, PartialEq)]
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
    pub error: Option<String>,
    /// Extracted lesson the next round should read.
    pub lesson: Option<String>,
    /// Optional embedding of the trial text, used for similarity search.
    pub embedding: Option<Vec<f32>>,
    /// Unix seconds when the trial was recorded. Zero means unset.
    pub recorded_at: i64,
}

impl Trial {
    pub(crate) fn from_parts(
        id: i64,
        scope: String,
        strategy: String,
        action: String,
        verdict: Verdict,
        summary: String,
        error: Option<String>,
        lesson: Option<String>,
        embedding: Option<Vec<f32>>,
        recorded_at: i64,
    ) -> Self {
        Self {
            id,
            scope,
            strategy,
            action,
            verdict,
            summary,
            error,
            lesson,
            embedding,
            recorded_at,
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
        parts.join(" ")
    }
}
