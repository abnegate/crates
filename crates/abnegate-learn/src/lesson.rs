//! Structured lesson extracted from trial text.

/// Structured lesson extracted from trial text.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Lesson {
    /// Root cause, when the text named one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_cause: Option<String>,
    /// Things the next round should not retry the same way.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub avoid: Vec<String>,
    /// Other decisions worth keeping.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    /// Source paths mentioned in the text, first seen first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Whether a test run was mentioned.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tests: bool,
    /// Named approach, when one could be inferred.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approach: Option<String>,
}

impl Lesson {
    /// An empty lesson.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether anything useful was extracted.
    pub fn is_empty(&self) -> bool {
        self.root_cause.is_none()
            && self.avoid.is_empty()
            && self.decisions.is_empty()
            && self.files.is_empty()
            && !self.tests
            && self
                .approach
                .as_deref()
                .is_none_or(|approach| approach == "unknown")
    }
}
