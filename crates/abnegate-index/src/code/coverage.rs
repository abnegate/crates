//! Turn source lines no chunk covers into top-level chunks.

use crate::code::OVERLAP_DIVISOR;
use crate::code::chunk::CodeChunk;
use crate::code::chunk_type::CodeChunkType;
use crate::code::config::ChunkConfig;
use crate::code::detect::compute_hash;
use crate::code::identifier_bag::IdentifierBag;
use crate::code::language::CodeLanguage;
use crate::code::metadata::ChunkMetadata;
use crate::code::symbol_kind::SymbolKind;
use crate::code::visibility::Visibility;
use crate::tokens::chunk_text;
use crate::tokens::estimate_tokens;

const MINIMUM_TOP_LEVEL_CHARACTERS: usize = 15;

const BLANK_LINES_ENDING_BLOCK: usize = 2;

pub(super) fn build_uncovered_chunks(
    content: &str,
    chunks: &[CodeChunk],
    config: &ChunkConfig,
    language: CodeLanguage,
) -> Vec<CodeChunk> {
    let line_starts = line_start_offsets(content);
    if line_starts.is_empty() {
        return Vec::new();
    }

    let mut covered = vec![false; line_starts.len()];
    for chunk in chunks {
        if matches!(
            chunk.chunk_type,
            CodeChunkType::Skeleton | CodeChunkType::ApiSurface
        ) {
            continue;
        }
        let start_line = byte_to_line(&line_starts, chunk.start_offset);
        let end_line = byte_to_line(&line_starts, chunk.end_offset.saturating_sub(1));
        for line in start_line..=end_line.min(covered.len().saturating_sub(1)) {
            covered[line] = true;
        }
    }

    let lines: Vec<&str> = content.lines().collect();
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    let mut block_start: Option<usize> = None;
    let mut blank_run = 0usize;

    for (index, line) in lines.iter().enumerate() {
        if covered.get(index).copied().unwrap_or(false) {
            if let Some(start) = block_start.take() {
                blocks.push((start, index));
            }
            blank_run = 0;
            continue;
        }
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run >= BLANK_LINES_ENDING_BLOCK
                && let Some(start) = block_start.take()
            {
                blocks.push((start, index.saturating_sub(1)));
            }
            continue;
        }
        blank_run = 0;
        if block_start.is_none() {
            block_start = Some(index);
        }
    }
    if let Some(start) = block_start {
        blocks.push((start, lines.len()));
    }

    let mut uncovered = Vec::new();
    for (start_line, end_line) in blocks {
        let text = lines[start_line..end_line].join("\n");
        if text.trim().len() < MINIMUM_TOP_LEVEL_CHARACTERS {
            continue;
        }
        let start_offset = *line_starts.get(start_line).unwrap_or(&0);
        let end_offset = if end_line < line_starts.len() {
            line_starts[end_line]
        } else {
            content.len()
        };
        let mut header = String::new();
        if let Some(ref path) = config.file_path {
            header.push_str(&format!("File: {path}\npath: {path}\n"));
        }
        header.push_str(&format!(
            "Language: {}\nkind: top_level\n\n",
            language.as_str()
        ));
        let pieces = if estimate_tokens(&text) > config.maximum_tokens {
            chunk_text(
                &text,
                config.maximum_tokens,
                (config.maximum_tokens / OVERLAP_DIVISOR).max(1),
            )
            .into_iter()
            .map(|piece| piece.text)
            .collect()
        } else {
            vec![text]
        };
        for piece in pieces {
            let mut chunk_body = header.clone();
            chunk_body.push_str(&piece);
            let content_hash = compute_hash(&chunk_body);
            uncovered.push(CodeChunk {
                index: 0,
                chunk_type: CodeChunkType::TopLevel,
                text: chunk_body,
                start_offset,
                end_offset,
                metadata: ChunkMetadata {
                    path: config.file_path.clone(),
                    symbol: None,
                    kind: Some(SymbolKind::Other),
                    visibility: Visibility::Private,
                    parents: Vec::new(),
                    module: None,
                    is_test: false,
                    is_generated: false,
                    start_byte: start_offset,
                    end_byte: end_offset,
                    content_hash,
                },
                identifiers: IdentifierBag::default(),
            });
        }
    }

    uncovered
}

fn line_start_offsets(content: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, byte) in content.bytes().enumerate() {
        if byte == b'\n' && index + 1 < content.len() {
            starts.push(index + 1);
        }
    }
    starts
}

fn byte_to_line(line_starts: &[usize], byte: usize) -> usize {
    match line_starts.binary_search(&byte) {
        Ok(index) => index,
        Err(index) => index.saturating_sub(1),
    }
}
