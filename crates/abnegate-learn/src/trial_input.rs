//! Builder for a trial the host is about to record.

use crate::verdict::Verdict;

/// What to record. Built with [`new`](Self::new) and `with_*`, then passed to
/// [`Memory::record`](crate::Memory::record).
#[derive(Debug, Clone, PartialEq)]
pub struct TrialInput {
    pub(crate) id: Option<i64>,
    pub(crate) scope: String,
    pub(crate) strategy: String,
    pub(crate) action: String,
    pub(crate) verdict: Verdict,
    pub(crate) summary: String,
    pub(crate) error: Option<String>,
    pub(crate) lesson: Option<String>,
    pub(crate) embedding: Option<Vec<f32>>,
    pub(crate) recorded_at: i64,
}

impl TrialInput {
    /// A trial in `scope` for `strategy`, with an empty verdict.
    pub fn new(scope: impl Into<String>, strategy: impl Into<String>) -> Self {
        Self {
            id: None,
            scope: scope.into(),
            strategy: strategy.into(),
            action: String::new(),
            verdict: Verdict::Empty,
            summary: String::new(),
            error: None,
            lesson: None,
            embedding: None,
            recorded_at: 0,
        }
    }

    /// Hydrate with a host-assigned identifier, such as a database row id.
    pub fn with_id(mut self, id: i64) -> Self {
        self.id = Some(id);
        self
    }

    /// Action taken under the strategy.
    pub fn with_action(mut self, action: impl Into<String>) -> Self {
        self.action = action.into();
        self
    }

    /// How the attempt turned out.
    pub fn with_verdict(mut self, verdict: Verdict) -> Self {
        self.verdict = verdict;
        self
    }

    /// Short description of what happened.
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = summary.into();
        self
    }

    /// Error or skip reason.
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        let error = error.into();
        self.error = if error.is_empty() { None } else { Some(error) };
        self
    }

    /// Lesson the next round should read.
    pub fn with_lesson(mut self, lesson: impl Into<String>) -> Self {
        let lesson = lesson.into();
        self.lesson = if lesson.is_empty() {
            None
        } else {
            Some(lesson)
        };
        self
    }

    /// Embedding of the trial text.
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = if embedding.is_empty() {
            None
        } else {
            Some(embedding)
        };
        self
    }

    /// Unix seconds when the trial happened. Zero lets
    /// [`Memory::record`](crate::Memory::record) stamp the current time.
    pub fn with_recorded_at(mut self, recorded_at: i64) -> Self {
        self.recorded_at = recorded_at;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_error_and_lesson_are_absent() {
        let input = TrialInput::new("lab", "fuzz")
            .with_error("")
            .with_lesson("")
            .with_embedding(Vec::new());
        assert!(input.error.is_none());
        assert!(input.lesson.is_none());
        assert!(input.embedding.is_none());
    }
}
