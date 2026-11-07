//! Build a symbol's identifier bag from its signature and body.

use std::sync::LazyLock;

use regex::Regex;

use crate::code::identifier_bag::IdentifierBag;
use crate::code::keyword::is_keyword;
use crate::code::language::CodeLanguage;
use crate::code::symbol::ParsedSymbol;

const MINIMUM_KEY_STRING_LENGTH: usize = 2;

const MAXIMUM_KEY_STRING_LENGTH: usize = 50;

const MAXIMUM_REFERENCED_TYPES: usize = 20;

const MAXIMUM_REFERENCED_FUNCTIONS: usize = 20;

const MAXIMUM_KEY_STRINGS: usize = 10;

static IDENTIFIER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[a-zA-Z_][a-zA-Z0-9_]*\b").expect("the identifier pattern is a valid regex")
});

static STRING_LITERAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"["']([^"']{{{MINIMUM_KEY_STRING_LENGTH},{MAXIMUM_KEY_STRING_LENGTH}}})["']"#
    ))
    .expect("the string literal pattern is a valid regex")
});

pub(super) fn extract_identifiers(symbol: &ParsedSymbol, language: CodeLanguage) -> IdentifierBag {
    let mut bag = IdentifierBag {
        defined_symbol: symbol.name.clone(),
        ..Default::default()
    };

    if let Some(parameters) = extract_parameters(&symbol.signature, language) {
        for matched in IDENTIFIER.find_iter(&parameters) {
            let identifier = matched.as_str().to_string();
            if !is_keyword(&identifier, language) && !bag.parameters.contains(&identifier) {
                bag.parameters.push(identifier);
            }
        }
    }

    for matched in IDENTIFIER.find_iter(&symbol.body) {
        let identifier = matched.as_str().to_string();
        if is_type_like(&identifier) {
            if !bag.referenced_types.contains(&identifier) {
                bag.referenced_types.push(identifier);
            }
        } else if is_function_call_like(&identifier, &symbol.body)
            && !bag.referenced_functions.contains(&identifier)
        {
            bag.referenced_functions.push(identifier);
        }
    }

    for captures in STRING_LITERAL.captures_iter(&symbol.body) {
        if let Some(matched) = captures.get(1) {
            let literal = matched.as_str();
            if (literal.starts_with('/')
                || literal.contains('.')
                || literal.contains('_')
                || literal.starts_with("on")
                || literal.starts_with("handle"))
                && bag.key_strings.len() < MAXIMUM_KEY_STRINGS
            {
                bag.key_strings.push(literal.to_string());
            }
        }
    }

    bag.referenced_types.truncate(MAXIMUM_REFERENCED_TYPES);
    bag.referenced_functions
        .truncate(MAXIMUM_REFERENCED_FUNCTIONS);
    bag.key_strings.truncate(MAXIMUM_KEY_STRINGS);

    bag
}

fn extract_parameters(signature: &str, _language: CodeLanguage) -> Option<String> {
    let start = signature.find('(')?;
    let end = signature.rfind(')')?;
    if start < end {
        Some(signature[start + 1..end].to_string())
    } else {
        None
    }
}

fn is_type_like(identifier: &str) -> bool {
    identifier
        .chars()
        .next()
        .is_some_and(|character| character.is_uppercase() && identifier.len() > 1)
}

fn is_function_call_like(identifier: &str, body: &str) -> bool {
    let pattern = format!("{}(", identifier);
    body.contains(&pattern)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::visibility::Visibility;

    #[test]
    fn identifiers_come_from_the_signature_and_body() {
        let symbol = ParsedSymbol {
            node_type: "function_item".to_string(),
            name: Some("calculate".to_string()),
            start_byte: 0,
            end_byte: 100,
            visibility: Visibility::Public,
            signature: "pub fn calculate(x: i32, y: i32) -> Result<i32, Error>".to_string(),
            docstring: None,
            body: r#"pub fn calculate(x: i32, y: i32) -> Result<i32, Error> {
                let result = helper(x, y);
                Ok(result)
            }"#
            .to_string(),
            parent_names: Vec::new(),
            nested_symbols: Vec::new(),
            is_public: true,
        };

        let identifiers = extract_identifiers(&symbol, CodeLanguage::Rust);

        assert_eq!(identifiers.defined_symbol, Some("calculate".to_string()));
        assert!(identifiers.parameters.contains(&"x".to_string()));
        assert!(identifiers.parameters.contains(&"y".to_string()));
        assert!(identifiers.referenced_types.contains(&"Result".to_string()));
        assert!(identifiers.referenced_types.contains(&"Error".to_string()));
        assert!(
            identifiers
                .referenced_functions
                .contains(&"helper".to_string())
        );
    }

    #[test]
    fn string_literal_pattern_matches_the_donor_pattern() {
        assert_eq!(STRING_LITERAL.as_str(), r#"["']([^"']{2,50})["']"#);
    }
}
