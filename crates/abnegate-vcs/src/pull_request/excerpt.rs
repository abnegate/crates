use crate::truncation;
use std::fmt;

/// The start of a text read up to a byte limit, and whether the text went on
/// past it.
///
/// Displayed, it is the text followed by [`Excerpt::MARKER`] when it was cut,
/// so a reader of the rendered text knows it holds only the start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Excerpt {
    /// What was kept.
    pub text: String,
    /// Whether the text went on past the limit.
    pub truncated: bool,
}

impl Excerpt {
    /// What a cut excerpt ends with when displayed, on a line of its own.
    pub const MARKER: &str = truncation::MARKER;
}

impl fmt::Display for Excerpt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)?;
        if self.truncated {
            formatter.write_str(Self::MARKER)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_excerpt_marks_only_a_cut_text() {
        let whole = Excerpt {
            text: "fn main() {}\n".to_string(),
            truncated: false,
        };
        assert_eq!(whole.to_string(), "fn main() {}\n");

        let cut = Excerpt {
            text: "fn main() {".to_string(),
            truncated: true,
        };
        assert_eq!(cut.to_string(), format!("fn main() {{{}", Excerpt::MARKER));

        let nothing_kept = Excerpt {
            text: String::new(),
            truncated: true,
        };
        assert_eq!(nothing_kept.to_string(), Excerpt::MARKER);

        assert_eq!(
            Excerpt::MARKER,
            truncation::MARKER,
            "an excerpt and a git diff summary end a cut text the same way"
        );
    }
}
