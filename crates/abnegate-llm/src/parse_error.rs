use thiserror::Error;

/// A configuration value that names nothing this crate knows.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("`{value}` is not a {kind}")]
pub struct ParseError {
    kind: &'static str,
    value: String,
}

impl ParseError {
    pub(crate) fn new(kind: &'static str, value: &str) -> Self {
        Self {
            kind,
            value: value.to_string(),
        }
    }

    /// What was being parsed, such as `selection strategy`.
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// The text that did not parse.
    pub fn value(&self) -> &str {
        &self.value
    }
}

#[cfg(test)]
mod tests {
    use super::ParseError;

    #[test]
    fn a_failure_names_the_value_and_what_it_was_meant_to_be() {
        let error = ParseError::new("cost strategy", "priciest");

        assert_eq!(error.to_string(), "`priciest` is not a cost strategy");
        assert_eq!(error.kind(), "cost strategy");
        assert_eq!(error.value(), "priciest");
    }
}
