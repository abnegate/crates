//! How a chunk relates to the identifiers a question names.

/// How a chunk relates to the identifiers extracted from a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentifierRole {
    /// The chunk publicly declares an identifier: `pub fn`, `pub struct` and
    /// the like.
    Exported,
    /// The chunk defines an identifier privately, names it as a tool, route,
    /// table, field, re-export or enum variant, or is the file named for it.
    Defined,
    /// The chunk or its URI mentions an identifier without defining it.
    Mention,
    /// The chunk does not refer to any identifier, or there were none.
    None,
}

impl IdentifierRole {
    /// Whether this role is [`Exported`](Self::Exported) or
    /// [`Defined`](Self::Defined).
    pub fn is_definition(self) -> bool {
        matches!(self, Self::Exported | Self::Defined)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exported_and_defined_are_definitions() {
        assert!(IdentifierRole::Exported.is_definition());
        assert!(IdentifierRole::Defined.is_definition());
        assert!(!IdentifierRole::Mention.is_definition());
        assert!(!IdentifierRole::None.is_definition());
    }
}
