//! Keep chunks under the embed-safe size, and fall back to text windows.

use crate::code::OVERLAP_DIVISOR;
use crate::code::chunk::CodeChunk;
use crate::code::chunk_type::CodeChunkType;
use crate::code::config::ChunkConfig;
use crate::code::detect::compute_hash;
use crate::code::identifier_bag::IdentifierBag;
use crate::code::metadata::ChunkMetadata;
use crate::code::visibility::Visibility;
use crate::tokens::chunk_text;

const CHARACTERS_PER_CHUNK_TOKEN: usize = 6;

const MINIMUM_CHUNK_CHARACTERS: usize = 512;

pub(super) fn enforce_maximum_chunk_size(
    chunks: Vec<CodeChunk>,
    config: &ChunkConfig,
) -> Vec<CodeChunk> {
    let maximum_characters = config
        .maximum_tokens
        .saturating_mul(CHARACTERS_PER_CHUNK_TOKEN)
        .max(MINIMUM_CHUNK_CHARACTERS);
    let mut out = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        if chunk.text.chars().count() <= maximum_characters {
            out.push(chunk);
            continue;
        }
        let overlap = (config.maximum_tokens / OVERLAP_DIVISOR).max(1);
        for piece in chunk_text(&chunk.text, config.maximum_tokens, overlap) {
            let mut split = chunk.clone();
            split.text = piece.text;
            split.start_offset = chunk.start_offset.saturating_add(piece.start_offset);
            split.end_offset = chunk.start_offset.saturating_add(piece.end_offset);
            split.metadata.start_byte = split.start_offset;
            split.metadata.end_byte = split.end_offset;
            split.metadata.content_hash = compute_hash(&split.text);
            out.push(split);
        }
    }
    out
}

pub(super) fn fallback_to_text_chunks(
    content: &str,
    config: &ChunkConfig,
    is_generated: bool,
) -> Vec<CodeChunk> {
    let overlap = config.maximum_tokens / OVERLAP_DIVISOR;
    let text_chunks = chunk_text(content, config.maximum_tokens, overlap);

    text_chunks
        .into_iter()
        .map(|text_chunk| {
            let mut text = String::new();
            if let Some(ref path) = config.file_path {
                text.push_str(&format!("File: {path}\npath: {path}\n"));
            }
            text.push_str(&text_chunk.text);
            let content_hash = compute_hash(&text);
            CodeChunk {
                index: text_chunk.index,
                chunk_type: CodeChunkType::TopLevel,
                text,
                start_offset: text_chunk.start_offset,
                end_offset: text_chunk.end_offset,
                metadata: ChunkMetadata {
                    path: config.file_path.clone(),
                    symbol: None,
                    kind: None,
                    visibility: Visibility::Private,
                    parents: Vec::new(),
                    module: None,
                    is_test: false,
                    is_generated,
                    start_byte: text_chunk.start_offset,
                    end_byte: text_chunk.end_offset,
                    content_hash,
                },
                identifiers: IdentifierBag::default(),
            }
        })
        .collect()
}
