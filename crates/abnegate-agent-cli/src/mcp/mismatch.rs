use std::fmt;

/// Why a server's entry in a configuration document does not describe a
/// server: the field holding a value of the wrong JSON type, or none when the
/// entry itself is not an object, and what it must hold. It quotes nothing
/// from the document, since a value in the wrong place may still be a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Mismatch {
    pub(crate) field: Option<&'static str>,
    pub(crate) expected: &'static str,
}

impl Mismatch {
    pub(crate) fn new(field: Option<&'static str>, expected: &'static str) -> Self {
        Self { field, expected }
    }
}

impl fmt::Display for Mismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.field {
            Some(field) => write!(formatter, "`{field}` must be {}", self.expected),
            None => write!(formatter, "the entry must be {}", self.expected),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Mismatch;

    #[test]
    fn a_mismatch_names_the_field_and_what_it_must_hold() {
        assert_eq!(
            Mismatch::new(Some("headers"), "an object of strings").to_string(),
            "`headers` must be an object of strings"
        );
        assert_eq!(
            Mismatch::new(None, "an object").to_string(),
            "the entry must be an object"
        );
    }
}
