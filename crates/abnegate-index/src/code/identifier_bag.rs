//! The identifiers a code chunk defines and references.

/// The identifiers a code chunk defines and references, so keyword search and
/// vector search see the same names.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct IdentifierBag {
    /// The symbol the chunk defines.
    pub defined_symbol: Option<String>,
    /// Parameter names from the signature.
    pub parameters: Vec<String>,
    /// Type names the body references.
    pub referenced_types: Vec<String>,
    /// Function and method names the body calls.
    pub referenced_functions: Vec<String>,
    /// String literals that look like keys, routes or events.
    pub key_strings: Vec<String>,
}
