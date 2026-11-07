//! A symbol parsed from the syntax tree.

use crate::code::visibility::Visibility;
use crate::tokens::estimate_tokens;

#[derive(Debug, Clone)]
pub(super) struct ParsedSymbol {
    pub(super) node_type: String,
    pub(super) name: Option<String>,
    pub(super) start_byte: usize,
    pub(super) end_byte: usize,
    pub(super) visibility: Visibility,
    pub(super) signature: String,
    pub(super) docstring: Option<String>,
    pub(super) body: String,
    pub(super) parent_names: Vec<String>,
    pub(super) nested_symbols: Vec<ParsedSymbol>,
    pub(super) is_public: bool,
}

impl ParsedSymbol {
    pub(super) fn token_count(&self) -> usize {
        estimate_tokens(&self.body)
    }
}
