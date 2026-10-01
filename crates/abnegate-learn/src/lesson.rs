//! Structured lesson extracted from trial text.

/// Structured lesson extracted from trial text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Lesson {
    /// Root cause, when the text named one.
    pub root_cause: Option<String>,
    /// Things the next round should not retry the same way.
    pub avoid: Vec<String>,
    /// Other decisions worth keeping.
    pub decisions: Vec<String>,
}

impl Lesson {
    /// An empty lesson.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether anything was extracted.
    pub fn is_empty(&self) -> bool {
        self.root_cause.is_none() && self.avoid.is_empty() && self.decisions.is_empty()
    }
}
