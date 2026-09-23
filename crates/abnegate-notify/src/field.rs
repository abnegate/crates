//! A labelled detail hung off a notification.

use crate::notification::clean;

/// One name/value pair, rendered as an embed field or a Slack section field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    name: String,
    value: String,
}

impl Field {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: clean(name.into()),
            value: clean(value.into()),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_keeps_its_name_and_value() {
        let field = Field::new("Branch", "main");
        assert_eq!(field.name(), "Branch");
        assert_eq!(field.value(), "main");
    }

    #[test]
    fn both_halves_are_sanitized() {
        let field = Field::new("\u{1b}[1mBranch\u{1b}[0m", "token=ghp_0123456789abcdefghij");
        assert_eq!(field.name(), "Branch");
        assert_eq!(field.value(), "token=[REDACTED]");
    }
}
