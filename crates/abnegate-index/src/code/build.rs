//! Build the text and metadata of each chunk view.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::code::chunk::CodeChunk;
use crate::code::chunk_type::CodeChunkType;
use crate::code::config::ChunkConfig;
use crate::code::detect::compute_hash;
use crate::code::detect::is_test_code;
use crate::code::file_context::FileContext;
use crate::code::identifier_bag::IdentifierBag;
use crate::code::identifiers::extract_identifiers;
use crate::code::language::CodeLanguage;
use crate::code::metadata::ChunkMetadata;
use crate::code::process::classify_symbol_kind;
use crate::code::symbol::ParsedSymbol;
use crate::code::visibility::Visibility;
use crate::tokens::estimate_tokens;

const MAXIMUM_IDENTIFIER_BAG_ITEMS: usize = 32;

const MAXIMUM_SKELETON_TYPES: usize = 5;

static TYPE_IDENTIFIER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Z][a-zA-Z0-9_]*\b").expect("the type identifier pattern is a valid regex")
});

pub(super) fn build_body_chunk(
    symbol: &ParsedSymbol,
    config: &ChunkConfig,
    language: CodeLanguage,
    file_context: &FileContext,
) -> CodeChunk {
    let mut text = String::new();

    text.push_str(&format_chunk_header(symbol, config, language));
    text.push('\n');

    if let Some(ref docstring) = symbol.docstring {
        text.push_str(docstring);
        text.push('\n');
    }

    let relevant = relevant_imports(&symbol.body, &file_context.imports, config);
    if !relevant.is_empty() {
        text.push_str("// Imports:\n");
        text.push_str(&relevant);
        text.push_str("\n\n");
    }

    text.push_str(&symbol.body);

    let identifiers = extract_identifiers(symbol, language);
    text.push_str(&format_identifier_bag(&identifiers));

    let content_hash = compute_hash(&text);

    CodeChunk {
        index: 0,
        chunk_type: CodeChunkType::Body,
        text,
        start_offset: symbol.start_byte,
        end_offset: symbol.end_byte,
        metadata: ChunkMetadata {
            path: config.file_path.clone(),
            symbol: symbol.name.clone(),
            kind: Some(classify_symbol_kind(symbol, language)),
            visibility: symbol.visibility.clone(),
            parents: symbol.parent_names.clone(),
            module: None,
            is_test: is_test_code(&symbol.body, language),
            is_generated: false,
            start_byte: symbol.start_byte,
            end_byte: symbol.end_byte,
            content_hash,
        },
        identifiers,
    }
}

pub(super) fn build_skeleton_chunk(
    symbol: &ParsedSymbol,
    config: &ChunkConfig,
    language: CodeLanguage,
) -> CodeChunk {
    let mut text = String::new();

    text.push_str(&format_chunk_header(symbol, config, language));
    text.push_str(" [skeleton]\n");

    if let Some(ref docstring) = symbol.docstring {
        text.push_str(docstring);
        text.push('\n');
    }

    text.push_str(&symbol.signature);

    let identifiers = extract_identifiers(symbol, language);
    text.push_str("\n\n// Key identifiers:\n");
    if let Some(ref name) = identifiers.defined_symbol {
        text.push_str(&format!("// defined: {}\n", name));
    }
    if !identifiers.parameters.is_empty() {
        text.push_str(&format!(
            "// params: {}\n",
            identifiers.parameters.join(", ")
        ));
    }
    if !identifiers.referenced_types.is_empty() {
        text.push_str(&format!(
            "// types: {}\n",
            identifiers
                .referenced_types
                .iter()
                .take(MAXIMUM_SKELETON_TYPES)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let content_hash = compute_hash(&text);

    CodeChunk {
        index: 0,
        chunk_type: CodeChunkType::Skeleton,
        text,
        start_offset: symbol.start_byte,
        end_offset: symbol.end_byte,
        metadata: ChunkMetadata {
            path: config.file_path.clone(),
            symbol: symbol.name.clone(),
            kind: Some(classify_symbol_kind(symbol, language)),
            visibility: symbol.visibility.clone(),
            parents: symbol.parent_names.clone(),
            module: None,
            is_test: false,
            is_generated: false,
            start_byte: symbol.start_byte,
            end_byte: symbol.end_byte,
            content_hash,
        },
        identifiers,
    }
}

pub(super) fn build_file_header_chunk(
    context: &FileContext,
    config: &ChunkConfig,
    _language: CodeLanguage,
) -> CodeChunk {
    let mut text = String::new();

    if let Some(ref path) = config.file_path {
        text.push_str(&format!("path: {}\n", path));
    }
    text.push_str("kind: file_header\n\n");

    if let Some(ref docstring) = context.file_docstring {
        text.push_str(docstring);
        text.push_str("\n\n");
    }

    if let Some(ref module) = context.module_declaration {
        text.push_str(module);
        text.push('\n');
    }

    if !context.imports.is_empty() {
        text.push_str(&context.imports);
    }

    let content_hash = compute_hash(&text);

    CodeChunk {
        index: 0,
        chunk_type: CodeChunkType::FileHeader,
        text,
        start_offset: context.import_start,
        end_offset: context.import_end,
        metadata: ChunkMetadata {
            path: config.file_path.clone(),
            symbol: None,
            kind: None,
            visibility: Visibility::Public,
            parents: Vec::new(),
            module: context.module_declaration.clone(),
            is_test: false,
            is_generated: false,
            start_byte: context.import_start,
            end_byte: context.import_end,
            content_hash,
        },
        identifiers: IdentifierBag::default(),
    }
}

pub(super) fn build_api_surface_chunk(
    public_symbols: &[&ParsedSymbol],
    config: &ChunkConfig,
    _language: CodeLanguage,
) -> Option<CodeChunk> {
    if public_symbols.is_empty() {
        return None;
    }

    let mut text = String::new();

    if let Some(ref path) = config.file_path {
        text.push_str(&format!("path: {}\n", path));
    }
    text.push_str("kind: api_surface\n\n");

    text.push_str("// Public API:\n\n");

    for symbol in public_symbols {
        if let Some(ref docstring) = symbol.docstring
            && let Some(first_line) = docstring.lines().next()
        {
            text.push_str(first_line);
            text.push('\n');
        }
        text.push_str(&symbol.signature);
        text.push_str("\n\n");
    }

    let content_hash = compute_hash(&text);

    let start = public_symbols
        .first()
        .map(|symbol| symbol.start_byte)
        .unwrap_or(0);
    let end = public_symbols
        .last()
        .map(|symbol| symbol.end_byte)
        .unwrap_or(0);

    Some(CodeChunk {
        index: 0,
        chunk_type: CodeChunkType::ApiSurface,
        text,
        start_offset: start,
        end_offset: end,
        metadata: ChunkMetadata {
            path: config.file_path.clone(),
            symbol: None,
            kind: None,
            visibility: Visibility::Public,
            parents: Vec::new(),
            module: None,
            is_test: false,
            is_generated: false,
            start_byte: start,
            end_byte: end,
            content_hash,
        },
        identifiers: IdentifierBag {
            defined_symbol: None,
            parameters: Vec::new(),
            referenced_types: public_symbols
                .iter()
                .filter_map(|symbol| symbol.name.clone())
                .collect(),
            referenced_functions: Vec::new(),
            key_strings: Vec::new(),
        },
    })
}

pub(super) fn format_chunk_header(
    symbol: &ParsedSymbol,
    config: &ChunkConfig,
    language: CodeLanguage,
) -> String {
    let mut header = String::new();

    if let Some(ref path) = config.file_path {
        header.push_str(&format!("File: {}\n", path));
        header.push_str(&format!("path: {}\n", path));
    }
    header.push_str(&format!("Language: {}\n", language.as_str()));

    if let Some(ref name) = symbol.name {
        let parent_prefix = if symbol.parent_names.is_empty() {
            String::new()
        } else {
            format!("{}.", symbol.parent_names.join("."))
        };
        header.push_str(&format!("symbol: {}{}\n", parent_prefix, name));
        header.push_str(&format!("Symbol: {}{}\n", parent_prefix, name));
    }

    let kind = classify_symbol_kind(symbol, language);
    header.push_str(&format!("kind: {:?}\n", kind));

    if !symbol.parent_names.is_empty() {
        header.push_str(&format!("Parent: {}\n", symbol.parent_names.join(".")));
        header.push_str(&format!("parents: {}\n", symbol.parent_names.join(".")));
    }

    if !symbol.signature.is_empty() {
        header.push_str(&format!("Signature: {}\n", symbol.signature.trim()));
    }

    header
}

fn format_identifier_bag(identifiers: &IdentifierBag) -> String {
    let mut text = String::new();

    if identifiers.defined_symbol.is_none()
        && identifiers.parameters.is_empty()
        && identifiers.referenced_types.is_empty()
        && identifiers.referenced_functions.is_empty()
    {
        return text;
    }

    text.push_str("\n\n// Identifiers:\n");

    if let Some(ref name) = identifiers.defined_symbol {
        text.push_str(&format!("// symbol: {}\n", name));
    }

    if !identifiers.parameters.is_empty() {
        text.push_str(&format!(
            "// params: {}\n",
            format_identifier_list(&identifiers.parameters)
        ));
    }

    if !identifiers.referenced_types.is_empty() {
        text.push_str(&format!(
            "// types: {}\n",
            format_identifier_list(&identifiers.referenced_types)
        ));
    }

    if !identifiers.referenced_functions.is_empty() {
        text.push_str(&format!(
            "// calls: {}\n",
            format_identifier_list(&identifiers.referenced_functions)
        ));
    }

    if !identifiers.key_strings.is_empty() {
        text.push_str(&format!(
            "// keys: {}\n",
            format_identifier_list(&identifiers.key_strings)
        ));
    }

    text
}

fn format_identifier_list(items: &[String]) -> String {
    if items.len() <= MAXIMUM_IDENTIFIER_BAG_ITEMS {
        return items.join(", ");
    }
    format!(
        "{}, \u{2026} ({} more)",
        items[..MAXIMUM_IDENTIFIER_BAG_ITEMS].join(", "),
        items.len() - MAXIMUM_IDENTIFIER_BAG_ITEMS
    )
}

fn relevant_imports(body: &str, all_imports: &str, config: &ChunkConfig) -> String {
    if all_imports.is_empty() {
        return String::new();
    }

    let body_identifiers: HashSet<&str> = TYPE_IDENTIFIER
        .find_iter(body)
        .map(|matched| matched.as_str())
        .collect();

    let mut relevant = Vec::new();
    let mut tokens = 0;

    for line in all_imports.lines() {
        if tokens >= config.maximum_import_tokens {
            break;
        }

        let is_relevant = body_identifiers
            .iter()
            .any(|identifier| line.contains(identifier));

        if is_relevant {
            tokens += estimate_tokens(line);
            relevant.push(line);
        }
    }

    if relevant.is_empty() {
        for line in all_imports.lines() {
            if tokens >= config.maximum_import_tokens {
                break;
            }
            tokens += estimate_tokens(line);
            relevant.push(line);
        }
    }

    relevant.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_lists_are_capped_with_an_ellipsis() {
        let many: Vec<String> = (0..80).map(|index| format!("Type{index}")).collect();
        assert!(format_identifier_list(&many).contains("48 more"));
        assert_eq!(format_identifier_list(&["A".into(), "B".into()]), "A, B");
    }

    #[test]
    fn capped_identifier_list_uses_the_horizontal_ellipsis_character() {
        let many: Vec<String> = (0..33).map(|index| format!("T{index}")).collect();
        assert!(format_identifier_list(&many).ends_with("T31, \u{2026} (1 more)"));
    }
}
