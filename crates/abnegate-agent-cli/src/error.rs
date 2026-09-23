//! The one failure newline framing can report.

use thiserror::Error;

/// A line grew past the cap without ever terminating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("one event exceeded {limit} bytes")]
pub struct Overlong {
    pub limit: usize,
}

#[cfg(test)]
mod tests {
    use super::Overlong;

    #[test]
    fn the_cap_is_named_in_the_message() {
        assert_eq!(
            Overlong { limit: 256 }.to_string(),
            "one event exceeded 256 bytes"
        );
    }
}
