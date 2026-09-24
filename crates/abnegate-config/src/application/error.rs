use thiserror::Error;

/// Why a name cannot be an [`Application`](crate::Application).
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum ApplicationError {
    /// The name is empty, starts with something other than an ASCII letter or
    /// digit, or contains a character other than an ASCII letter, digit, `-`
    /// or `_`.
    #[error(
        "Application name {name:?} must start with an ASCII letter or digit and contain only ASCII letters, digits, '-' and '_'"
    )]
    Invalid {
        /// The refused name, exactly as it was given.
        name: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_name_is_quoted_with_its_control_characters_escaped() {
        let error = ApplicationError::Invalid {
            name: "example\0\nforged".to_string(),
        };

        assert_eq!(
            error.to_string(),
            "Application name \"example\\0\\nforged\" must start with an ASCII letter or digit and contain only ASCII letters, digits, '-' and '_'"
        );
    }
}
