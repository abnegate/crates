//! A statement the walk rule refuses.

use std::fmt;

/// A statement the walk rule refuses, and why.
pub(super) struct Refusal<'a> {
    /// The scanner file it is in, by its path under `src`.
    pub(super) file: &'a str,
    /// The line it starts on.
    pub(super) line: usize,
    pub(super) code: String,
    pub(super) reasons: Vec<String>,
}

impl fmt::Display for Refusal<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "src/{}:{} {}: {}",
            self.file,
            self.line,
            self.reasons.join("; "),
            self.code
        )
    }
}
