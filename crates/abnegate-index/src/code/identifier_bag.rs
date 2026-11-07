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

impl IdentifierBag {
    /// Set the symbol the chunk defines.
    pub fn with_defined_symbol(mut self, defined_symbol: impl Into<String>) -> Self {
        self.defined_symbol = Some(defined_symbol.into());
        self
    }

    /// Set the parameter names from the signature.
    pub fn with_parameters(mut self, parameters: Vec<String>) -> Self {
        self.parameters = parameters;
        self
    }

    /// Set the type names the body references.
    pub fn with_referenced_types(mut self, referenced_types: Vec<String>) -> Self {
        self.referenced_types = referenced_types;
        self
    }

    /// Set the function and method names the body calls.
    pub fn with_referenced_functions(mut self, referenced_functions: Vec<String>) -> Self {
        self.referenced_functions = referenced_functions;
        self
    }

    /// Set the string literals that look like keys, routes or events.
    pub fn with_key_strings(mut self, key_strings: Vec<String>) -> Self {
        self.key_strings = key_strings;
        self
    }
}
