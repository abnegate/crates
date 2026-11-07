//! Turn parsed symbols into body and skeleton chunks, merging small symbols
//! and splitting large ones.

use std::collections::HashSet;

use crate::code::build::build_body_chunk;
use crate::code::build::build_skeleton_chunk;
use crate::code::build::format_chunk_header;
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
use crate::code::symbol::ParsedSymbol;
use crate::code::symbol_kind::SymbolKind;
use crate::code::visibility::Visibility;
use crate::tokens::estimate_tokens;

const LARGE_CONTAINER_LINES: usize = 100;

const SPLIT_OVERLAP_RESERVE_TOKENS: usize = 50;

const SPLIT_OVERLAP_LINES: usize = 2;

/// The chunks of `symbols`, in source order.
///
/// `expanded` holds the symbols already split for being over the token budget.
/// A symbol is reached once through its parent's split and again through its
/// parent's nested symbols, which doubles the work at every level of nesting.
/// Expanding it again could only repeat chunks already emitted, which
/// [`chunk_code_with_config`](super::chunk_code_with_config) drops by content
/// hash, so a later visit emits nothing.
pub(super) fn process_symbols(
    symbols: &[ParsedSymbol],
    source: &str,
    config: &ChunkConfig,
    language: CodeLanguage,
    file_context: &FileContext,
    expanded: &mut HashSet<*const ParsedSymbol>,
) -> Vec<CodeChunk> {
    let mut chunks = Vec::new();
    let mut pending_small: Vec<&ParsedSymbol> = Vec::new();

    for symbol in symbols {
        let token_count = symbol.token_count();
        let line_count = symbol.body.lines().count();
        let is_container = language.is_container_node(&symbol.node_type);
        let large_container = is_container
            && !symbol.nested_symbols.is_empty()
            && (token_count > config.maximum_tokens || line_count > LARGE_CONTAINER_LINES);

        let kind = classify_symbol_kind(symbol, language);
        let keep_named_callable =
            matches!(kind, SymbolKind::Function | SymbolKind::Method) && symbol.name.is_some();
        if token_count < config.minimum_tokens && !large_container && !keep_named_callable {
            pending_small.push(symbol);
            continue;
        }

        if !pending_small.is_empty() {
            chunks.extend(merge_small_symbols(
                &pending_small,
                config,
                language,
                file_context,
            ));
            pending_small.clear();
        }

        if large_container {
            if config.generate_skeletons {
                chunks.push(build_skeleton_chunk(symbol, config, language));
            }
            chunks.extend(process_symbols(
                &symbol.nested_symbols,
                source,
                config,
                language,
                file_context,
                expanded,
            ));
            continue;
        }

        if token_count <= config.maximum_tokens {
            chunks.push(build_body_chunk(symbol, config, language, file_context));

            if config.generate_skeletons && !is_container {
                chunks.push(build_skeleton_chunk(symbol, config, language));
            }
        } else if expanded.insert(std::ptr::from_ref(symbol)) {
            chunks.extend(split_large_symbol(
                symbol,
                source,
                config,
                language,
                file_context,
                expanded,
            ));
        } else {
            continue;
        }

        if !symbol.nested_symbols.is_empty() {
            chunks.extend(process_symbols(
                &symbol.nested_symbols,
                source,
                config,
                language,
                file_context,
                expanded,
            ));
        }
    }

    if !pending_small.is_empty() {
        chunks.extend(merge_small_symbols(
            &pending_small,
            config,
            language,
            file_context,
        ));
    }

    chunks
}

fn merge_small_symbols(
    symbols: &[&ParsedSymbol],
    config: &ChunkConfig,
    language: CodeLanguage,
    file_context: &FileContext,
) -> Vec<CodeChunk> {
    if symbols.is_empty() {
        return Vec::new();
    }

    if symbols.len() == 1 {
        return vec![build_body_chunk(symbols[0], config, language, file_context)];
    }

    let mut by_parent: Vec<(Vec<String>, Vec<&ParsedSymbol>)> = Vec::new();
    for symbol in symbols {
        match by_parent
            .iter_mut()
            .find(|(parent_names, _)| *parent_names == symbol.parent_names)
        {
            Some((_, group)) => group.push(*symbol),
            None => by_parent.push((symbol.parent_names.clone(), vec![*symbol])),
        }
    }

    let mut chunks = Vec::new();

    for (parent_names, group) in by_parent {
        let mut combined_text = String::new();
        let mut combined_identifiers = IdentifierBag::default();
        let mut total_tokens = 0;

        let start_byte = group.first().map(|symbol| symbol.start_byte).unwrap_or(0);
        let mut end_byte = start_byte;

        for symbol in &group {
            let symbol_tokens = symbol.token_count();

            if total_tokens + symbol_tokens > config.maximum_tokens && !combined_text.is_empty() {
                let content_hash = compute_hash(&combined_text);
                chunks.push(CodeChunk {
                    index: 0,
                    chunk_type: CodeChunkType::Body,
                    text: combined_text.clone(),
                    start_offset: start_byte,
                    end_offset: end_byte,
                    metadata: ChunkMetadata {
                        path: config.file_path.clone(),
                        symbol: None,
                        kind: Some(SymbolKind::Other),
                        visibility: Visibility::Private,
                        parents: parent_names.clone(),
                        module: None,
                        is_test: false,
                        is_generated: false,
                        start_byte,
                        end_byte,
                        content_hash,
                    },
                    identifiers: combined_identifiers.clone(),
                });

                combined_text.clear();
                combined_identifiers = IdentifierBag::default();
                total_tokens = 0;
            }

            if !combined_text.is_empty() {
                combined_text.push_str("\n\n");
            }

            if let Some(ref docstring) = symbol.docstring {
                combined_text.push_str(docstring);
                combined_text.push('\n');
            }
            combined_text.push_str(&symbol.body);

            let identifiers = extract_identifiers(symbol, language);
            if let Some(name) = identifiers.defined_symbol {
                combined_identifiers.referenced_functions.push(name);
            }
            combined_identifiers
                .parameters
                .extend(identifiers.parameters);
            combined_identifiers
                .referenced_types
                .extend(identifiers.referenced_types);

            total_tokens += symbol_tokens;
            end_byte = symbol.end_byte;
        }

        if !combined_text.is_empty() {
            let content_hash = compute_hash(&combined_text);
            chunks.push(CodeChunk {
                index: 0,
                chunk_type: CodeChunkType::Body,
                text: combined_text,
                start_offset: start_byte,
                end_offset: end_byte,
                metadata: ChunkMetadata {
                    path: config.file_path.clone(),
                    symbol: None,
                    kind: Some(SymbolKind::Other),
                    visibility: Visibility::Private,
                    parents: parent_names,
                    module: None,
                    is_test: false,
                    is_generated: false,
                    start_byte,
                    end_byte,
                    content_hash,
                },
                identifiers: combined_identifiers,
            });
        }
    }

    chunks
}

fn split_large_symbol(
    symbol: &ParsedSymbol,
    source: &str,
    config: &ChunkConfig,
    language: CodeLanguage,
    file_context: &FileContext,
    expanded: &mut HashSet<*const ParsedSymbol>,
) -> Vec<CodeChunk> {
    let mut chunks = Vec::new();

    let mut header = String::new();
    header.push_str(&format_chunk_header(symbol, config, language));
    header.push('\n');
    if let Some(ref docstring) = symbol.docstring {
        header.push_str(docstring);
        header.push('\n');
    }
    header.push_str(&symbol.signature);
    header.push_str(" {\n");

    let header_tokens = estimate_tokens(&header);
    let body_budget = config
        .maximum_tokens
        .saturating_sub(header_tokens)
        .saturating_sub(SPLIT_OVERLAP_RESERVE_TOKENS);

    if !symbol.nested_symbols.is_empty() {
        for nested in &symbol.nested_symbols {
            chunks.extend(process_symbols(
                std::slice::from_ref(nested),
                source,
                config,
                language,
                file_context,
                expanded,
            ));
        }
        return chunks;
    }

    let body_text = source
        .get(symbol.start_byte..symbol.end_byte)
        .unwrap_or(&symbol.body);

    let lines: Vec<&str> = body_text.lines().collect();
    let mut current_chunk = header.clone();
    let mut current_start = symbol.start_byte;
    let mut chunk_index = 0;

    for (line_index, line) in lines.iter().enumerate() {
        let line_tokens = estimate_tokens(line);

        if estimate_tokens(&current_chunk) + line_tokens > body_budget && !current_chunk.is_empty()
        {
            let content_hash = compute_hash(&current_chunk);
            chunks.push(CodeChunk {
                index: chunk_index,
                chunk_type: CodeChunkType::Body,
                text: current_chunk.clone(),
                start_offset: current_start,
                end_offset: symbol.start_byte + body_text[..].find(line).unwrap_or(0),
                metadata: ChunkMetadata {
                    path: config.file_path.clone(),
                    symbol: symbol.name.clone(),
                    kind: Some(SymbolKind::from_node_type(&symbol.node_type, language)),
                    visibility: symbol.visibility.clone(),
                    parents: symbol.parent_names.clone(),
                    module: None,
                    is_test: is_test_code(&current_chunk, language),
                    is_generated: false,
                    start_byte: current_start,
                    end_byte: symbol.end_byte,
                    content_hash,
                },
                identifiers: extract_identifiers(symbol, language),
            });

            current_chunk = header.clone();
            let overlap_start = line_index.saturating_sub(SPLIT_OVERLAP_LINES);
            for previous_line in &lines[overlap_start..line_index] {
                current_chunk.push_str(previous_line);
                current_chunk.push('\n');
            }
            current_start =
                symbol.start_byte + body_text[..].find(lines[overlap_start]).unwrap_or(0);
            chunk_index += 1;
        }

        current_chunk.push_str(line);
        current_chunk.push('\n');
    }

    if !current_chunk.is_empty() && current_chunk != header {
        let content_hash = compute_hash(&current_chunk);
        chunks.push(CodeChunk {
            index: chunk_index,
            chunk_type: CodeChunkType::Body,
            text: current_chunk,
            start_offset: current_start,
            end_offset: symbol.end_byte,
            metadata: ChunkMetadata {
                path: config.file_path.clone(),
                symbol: symbol.name.clone(),
                kind: Some(SymbolKind::from_node_type(&symbol.node_type, language)),
                visibility: symbol.visibility.clone(),
                parents: symbol.parent_names.clone(),
                module: None,
                is_test: is_test_code(&symbol.body, language),
                is_generated: false,
                start_byte: current_start,
                end_byte: symbol.end_byte,
                content_hash,
            },
            identifiers: extract_identifiers(symbol, language),
        });
    }

    chunks
}

pub(super) fn classify_symbol_kind(symbol: &ParsedSymbol, language: CodeLanguage) -> SymbolKind {
    let kind = SymbolKind::from_node_type(&symbol.node_type, language);
    if !symbol.parent_names.is_empty() && kind == SymbolKind::Function {
        SymbolKind::Method
    } else {
        kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constant(name: &str, parents: &[&str], start_byte: usize) -> ParsedSymbol {
        let body = format!("const {name}: i32 = 1;");
        ParsedSymbol {
            node_type: "const_item".to_string(),
            name: Some(name.to_string()),
            start_byte,
            end_byte: start_byte + body.len(),
            visibility: Visibility::Private,
            signature: body.clone(),
            docstring: None,
            body,
            parent_names: parents.iter().map(|parent| parent.to_string()).collect(),
            nested_symbols: Vec::new(),
            is_public: false,
        }
    }

    #[test]
    fn siblings_with_different_parents_merge_in_first_seen_order() {
        let symbols = [
            constant("ZETA", &["Zeta"], 0),
            constant("ALPHA", &["Alpha"], 100),
            constant("OMEGA", &["Zeta"], 200),
            constant("BETA", &["Alpha"], 300),
            constant("ROOT", &[], 400),
        ];
        let references: Vec<&ParsedSymbol> = symbols.iter().collect();

        for _ in 0..16 {
            let chunks = merge_small_symbols(
                &references,
                &ChunkConfig::default(),
                CodeLanguage::Rust,
                &FileContext::default(),
            );
            let parents: Vec<Vec<String>> = chunks
                .iter()
                .map(|chunk| chunk.metadata.parents.clone())
                .collect();
            assert_eq!(
                parents,
                [
                    vec!["Zeta".to_string()],
                    vec!["Alpha".to_string()],
                    Vec::new(),
                ]
            );
            assert_eq!(
                chunks[0].text,
                "const ZETA: i32 = 1;\n\nconst OMEGA: i32 = 1;"
            );
            assert_eq!(chunks[1].start_offset, 100);
            assert_eq!(chunks[1].end_offset, 300 + "const BETA: i32 = 1;".len());
        }
    }
}
