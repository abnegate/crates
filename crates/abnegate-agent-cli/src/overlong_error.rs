//! The one failure newline framing can report.

use thiserror::Error;

/// A line grew past the cap.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("one event exceeded {limit} bytes")]
#[non_exhaustive]
pub struct OverlongError {
    /// The cap the line broke, in bytes.
    pub limit: usize,
    /// The start of the line, enough to tell what kind of event it was.
    pub prefix: String,
}

#[cfg(test)]
mod tests {
    use super::OverlongError;

    #[test]
    fn the_cap_is_named_in_the_message() {
        assert_eq!(
            OverlongError {
                limit: 256,
                prefix: "{\"type\":\"user\"".to_string(),
            }
            .to_string(),
            "one event exceeded 256 bytes"
        );
    }
}
