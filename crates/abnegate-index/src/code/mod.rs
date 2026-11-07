//! Code-aware chunking with tree-sitter.
//!
//! Source is split on symbol boundaries, never at a fixed length. Each symbol
//! gets one body chunk and, unless it is a container, a skeleton chunk; small
//! siblings are merged and large symbols are split with header overlap. A file
//! header chunk carries the imports, an API surface chunk lists the public
//! signatures, and source no symbol covers becomes top-level chunks. Every
//! chunk carries an identifier bag so keyword and vector search see the same
//! names.

mod build;
mod chunk;
mod chunk_type;
mod config;
mod coverage;
mod detect;
mod file_context;
mod identifier_bag;
mod identifiers;
mod keyword;
mod language;
mod metadata;
mod parse;
mod process;
mod size;
mod symbol;
mod symbol_kind;
mod visibility;

use std::collections::HashSet;

use crate::code::build::build_api_surface_chunk;
use crate::code::build::build_file_header_chunk;
use crate::code::coverage::build_uncovered_chunks;
use crate::code::detect::is_generated_path;
use crate::code::detect::is_likely_generated_or_minified;
use crate::code::parse::extract_file_context;
use crate::code::parse::extract_symbols;
use crate::code::parse::parse_code;
use crate::code::process::process_symbols;
use crate::code::size::enforce_maximum_chunk_size;
use crate::code::size::fallback_to_text_chunks;
use crate::text_chunk::TextChunk;
use crate::tokens::chunk_text;

pub use crate::code::chunk::CodeChunk;
pub use crate::code::chunk_type::CodeChunkType;
pub use crate::code::config::ChunkConfig;
pub use crate::code::identifier_bag::IdentifierBag;
pub use crate::code::language::CodeLanguage;
pub use crate::code::metadata::ChunkMetadata;
pub use crate::code::symbol_kind::SymbolKind;
pub use crate::code::visibility::Visibility;

const OVERLAP_DIVISOR: usize = 10;

/// Chunk `content` as `language` with the default [`ChunkConfig`] and a budget
/// of `maximum_tokens` tokens a chunk.
///
/// The budget is a target, not a limit: a chunk can be over it, up to the larger
/// of 6 characters a token and 512 characters, past which it is cut into text
/// windows.
pub fn chunk_code(content: &str, language: CodeLanguage, maximum_tokens: usize) -> Vec<CodeChunk> {
    chunk_code_with_config(
        content,
        language,
        &ChunkConfig {
            maximum_tokens,
            ..Default::default()
        },
    )
}

/// Chunk `content` as `language` with `config`.
///
/// Content that looks generated or minified, sits under a generated path, or
/// does not parse falls back to plain text windows. Chunks with identical
/// text are emitted once, and indexes count from 0 in emission order.
pub fn chunk_code_with_config(
    content: &str,
    language: CodeLanguage,
    config: &ChunkConfig,
) -> Vec<CodeChunk> {
    if is_likely_generated_or_minified(content)
        || config.file_path.as_deref().is_some_and(is_generated_path)
    {
        return fallback_to_text_chunks(content, config, true);
    }

    let tree = match parse_code(content, language) {
        Some(tree) => tree,
        None => {
            return fallback_to_text_chunks(content, config, false);
        }
    };

    let source = content.as_bytes();
    let root = tree.root_node();

    let file_context = extract_file_context(&root, source, language);

    let symbols = extract_symbols(&root, source, language, Vec::new());

    let mut chunks = Vec::new();
    let mut seen_hashes = HashSet::new();

    if !file_context.imports.is_empty() || file_context.module_declaration.is_some() {
        let header_chunk = build_file_header_chunk(&file_context, config, language);
        if !seen_hashes.contains(&header_chunk.metadata.content_hash) {
            seen_hashes.insert(header_chunk.metadata.content_hash.clone());
            chunks.push(header_chunk);
        }
    }

    let mut expanded = HashSet::new();
    let processed = process_symbols(
        &symbols,
        content,
        config,
        language,
        &file_context,
        &mut expanded,
    );

    for chunk in processed {
        if !seen_hashes.contains(&chunk.metadata.content_hash) {
            seen_hashes.insert(chunk.metadata.content_hash.clone());
            chunks.push(chunk);
        }
    }

    if config.generate_api_surface {
        let public_symbols: Vec<_> = symbols.iter().filter(|symbol| symbol.is_public).collect();
        if !public_symbols.is_empty()
            && let Some(api_chunk) = build_api_surface_chunk(&public_symbols, config, language)
            && !seen_hashes.contains(&api_chunk.metadata.content_hash)
        {
            seen_hashes.insert(api_chunk.metadata.content_hash.clone());
            chunks.push(api_chunk);
        }
    }

    let uncovered = build_uncovered_chunks(content, &chunks, config, language);
    for chunk in uncovered {
        if !seen_hashes.contains(&chunk.metadata.content_hash) {
            seen_hashes.insert(chunk.metadata.content_hash.clone());
            chunks.push(chunk);
        }
    }

    if chunks.is_empty() {
        return fallback_to_text_chunks(content, config, false);
    }

    chunks = enforce_maximum_chunk_size(chunks, config);

    for (index, chunk) in chunks.iter_mut().enumerate() {
        chunk.index = index;
    }

    chunks
}

/// The plain [`TextChunk`] view of `code_chunk`: its index, text and offsets.
pub fn code_chunk_to_text_chunk(code_chunk: CodeChunk) -> TextChunk {
    TextChunk {
        index: code_chunk.index,
        text: code_chunk.text,
        start_offset: code_chunk.start_offset,
        end_offset: code_chunk.end_offset,
    }
}

/// Chunk `content` with the code chunker when its language is known, and as
/// plain text windows otherwise.
///
/// The language comes from `extension` when one is given, and from
/// `content_type` when not. `file_path` is written into code chunk headers.
/// `overlap_tokens` applies only to plain text windows.
pub fn smart_chunk(
    content: &str,
    content_type: &str,
    extension: Option<&str>,
    file_path: Option<&str>,
    maximum_tokens: usize,
    overlap_tokens: usize,
) -> Vec<TextChunk> {
    let language = if let Some(extension) = extension {
        CodeLanguage::from_extension(extension)
    } else {
        CodeLanguage::from_content_type(content_type)
    };

    match language {
        CodeLanguage::Unknown => chunk_text(content, maximum_tokens, overlap_tokens),
        _ => {
            let code_chunks = chunk_code_with_config(
                content,
                language,
                &ChunkConfig {
                    maximum_tokens,
                    file_path: file_path.map(str::to_string),
                    ..ChunkConfig::default()
                },
            );
            code_chunks
                .into_iter()
                .map(code_chunk_to_text_chunk)
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_code_gets_a_file_header_and_body_chunks() {
        let code = r#"
use std::collections::HashMap;

/// A simple struct
pub struct Point {
    x: i32,
    y: i32,
}

impl Point {
    /// Create a new point
    pub fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    fn distance(&self) -> f64 {
        ((self.x * self.x + self.y * self.y) as f64).sqrt()
    }
}

fn helper() {
    println!("helper");
}
"#;

        let chunks = chunk_code(code, CodeLanguage::Rust, 512);

        assert!(!chunks.is_empty());

        let has_header = chunks
            .iter()
            .any(|chunk| chunk.chunk_type == CodeChunkType::FileHeader);
        assert!(has_header, "Should have file header chunk");

        let has_body = chunks
            .iter()
            .any(|chunk| chunk.chunk_type == CodeChunkType::Body);
        assert!(has_body, "Should have body chunks");

        for chunk in &chunks {
            assert!(!chunk.metadata.content_hash.is_empty());
        }
    }

    #[test]
    fn python_classes_and_functions_become_body_chunks() {
        let code = r#"
import os
from typing import List

class Calculator:
    """A simple calculator class."""

    def __init__(self):
        self.result = 0

    def add(self, x: int, y: int) -> int:
        """Add two numbers."""
        return x + y

def main():
    calc = Calculator()
    print(calc.add(1, 2))
"#;

        let chunks = chunk_code(code, CodeLanguage::Python, 512);
        assert!(!chunks.is_empty());

        let has_body = chunks
            .iter()
            .any(|chunk| chunk.chunk_type == CodeChunkType::Body);
        assert!(has_body);
    }

    #[test]
    fn kotlin_code_is_chunked() {
        let code = r#"
package com.example

import kotlin.math.sqrt

data class Point(val x: Int, val y: Int) {
    fun distance(): Double = sqrt((x * x + y * y).toDouble())
}

fun main() {
    val p = Point(3, 4)
    println(p.distance())
}
"#;

        let chunks = chunk_code(code, CodeLanguage::Kotlin, 512);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn swift_code_is_chunked() {
        let code = r#"
import Foundation

struct Point {
    let x: Int
    let y: Int

    func distance() -> Double {
        return sqrt(Double(x * x + y * y))
    }
}

func main() {
    let p = Point(x: 3, y: 4)
    print(p.distance())
}
"#;

        let chunks = chunk_code(code, CodeLanguage::Swift, 512);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn sql_code_is_chunked() {
        let code = r#"
CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    email VARCHAR(255) UNIQUE
);

CREATE INDEX idx_users_email ON users(email);

SELECT * FROM users WHERE email = 'test@example.com';
"#;

        let chunks = chunk_code(code, CodeLanguage::Sql, 512);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn a_large_enough_function_gets_a_skeleton_chunk() {
        let code = r#"
/// A public function that greets users with a personalized message
/// based on their name and some additional context information
pub fn hello(name: &str, greeting_style: &str, include_timestamp: bool) -> String {
    let base_greeting = if greeting_style == "formal" {
        format!("Good day, dear {}", name)
    } else if greeting_style == "casual" {
        format!("Hey there, {}", name)
    } else {
        format!("Hello, {}", name)
    };

    if include_timestamp {
        format!("{} at some time", base_greeting)
    } else {
        base_greeting
    }
}
"#;

        let config = ChunkConfig::default()
            .with_maximum_tokens(512)
            .with_minimum_tokens(20)
            .with_generate_skeletons(true);

        let chunks = chunk_code_with_config(code, CodeLanguage::Rust, &config);

        let has_skeleton = chunks
            .iter()
            .any(|chunk| chunk.chunk_type == CodeChunkType::Skeleton);
        assert!(has_skeleton, "Should generate skeleton chunk");
    }

    #[test]
    fn api_surface_lists_only_public_symbols() {
        let code = r#"
pub fn public_fn() {}
fn private_fn() {}
pub struct PublicStruct {}
"#;

        let config = ChunkConfig::default()
            .with_maximum_tokens(512)
            .with_generate_api_surface(true);

        let chunks = chunk_code_with_config(code, CodeLanguage::Rust, &config);

        let api_chunk = chunks
            .iter()
            .find(|chunk| chunk.chunk_type == CodeChunkType::ApiSurface);
        assert!(api_chunk.is_some(), "Should have API surface chunk");

        let api = api_chunk.unwrap();
        assert!(api.text.contains("public_fn"));
        assert!(api.text.contains("PublicStruct"));
        assert!(!api.text.contains("private_fn"));
    }

    #[test]
    fn a_large_function_splits_into_chunks_that_keep_its_context() {
        let large_body = "    println!(\"line\");\n".repeat(500);
        let code = format!(
            r#"
fn large_function() {{
{}
}}
"#,
            large_body
        );

        let chunks = chunk_code(&code, CodeLanguage::Rust, 100);

        assert!(chunks.len() > 1, "Large function should be split");

        for chunk in &chunks {
            if chunk.chunk_type == CodeChunkType::Body {
                assert!(
                    chunk.text.contains("large_function") || chunk.text.contains("path:"),
                    "Chunks should have context"
                );
            }
        }
    }

    #[test]
    fn small_symbols_merge_into_fewer_chunks() {
        let code = r#"
const A: i32 = 1;
const B: i32 = 2;
const C: i32 = 3;
"#;

        let config = ChunkConfig::default()
            .with_maximum_tokens(512)
            .with_minimum_tokens(50);

        let chunks = chunk_code_with_config(code, CodeLanguage::Rust, &config);

        let body_chunks: Vec<_> = chunks
            .iter()
            .filter(|chunk| chunk.chunk_type == CodeChunkType::Body)
            .collect();

        assert!(body_chunks.len() <= 2);
    }

    #[test]
    fn symbol_chunks_carry_the_file_path() {
        let code = r#"
pub fn hello() {
    println!("hello");
}
"#;

        let config = ChunkConfig::default().with_file_path("src/lib.rs");

        let chunks = chunk_code_with_config(code, CodeLanguage::Rust, &config);

        for chunk in &chunks {
            if chunk.metadata.symbol.is_some() {
                assert_eq!(chunk.metadata.path, Some("src/lib.rs".to_string()));
            }
        }
    }

    #[test]
    fn identical_chunks_are_emitted_once() {
        let code = r#"
fn same() {}
fn same() {}
"#;

        let chunks = chunk_code(code, CodeLanguage::Rust, 512);

        let mut hashes = HashSet::new();
        for chunk in &chunks {
            assert!(
                !hashes.contains(&chunk.metadata.content_hash),
                "Should not have duplicate chunks"
            );
            hashes.insert(chunk.metadata.content_hash.clone());
        }
    }

    #[test]
    fn impl_methods_are_extracted() {
        let code = r#"
pub struct Point {
    x: i32,
}

impl Point {
    pub fn new(x: i32) -> Self {
        Self { x }
    }

    pub fn magnitude(&self) -> i32 {
        self.x
    }
}

fn standalone() {}
"#;
        let chunks = chunk_code_with_config(
            code,
            CodeLanguage::Rust,
            &ChunkConfig::default().with_file_path("src/point.rs"),
        );

        let symbols: Vec<_> = chunks
            .iter()
            .filter_map(|chunk| chunk.metadata.symbol.as_deref())
            .collect();
        assert!(
            symbols
                .iter()
                .any(|name| name.contains("new") || *name == "new"),
            "impl methods should be extracted, got {symbols:?}"
        );
        assert!(
            chunks.iter().any(|chunk| {
                chunk.text.contains("Language: rust") && chunk.text.contains("src/point.rs")
            }),
            "embedding text should include path and language"
        );
    }

    #[test]
    fn uncovered_top_level_statements_become_top_level_chunks() {
        let code = r#"
fn ready() {}

let startup = initialize();
run(startup);
"#;
        let chunks = chunk_code(code, CodeLanguage::Rust, 512);
        let top_level: Vec<_> = chunks
            .iter()
            .filter(|chunk| chunk.chunk_type == CodeChunkType::TopLevel)
            .collect();
        assert!(
            top_level
                .iter()
                .any(|chunk| chunk.text.contains("initialize")),
            "uncovered executable lines should become top-level chunks: {top_level:?}"
        );
    }

    #[test]
    fn tsx_and_php_are_parsed() {
        let tsx = r#"
export function Hello({ name }: { name: string }) {
  return <div>Hello {name}</div>;
}
"#;
        let tsx_chunks = chunk_code(tsx, CodeLanguage::Tsx, 512);
        assert!(!tsx_chunks.is_empty());

        let php = r#"
<?php
class Greeter {
    public function hello($name) {
        return "Hello $name";
    }
}
"#;
        let php_chunks = chunk_code(php, CodeLanguage::Php, 512);
        assert!(!php_chunks.is_empty());
        assert!(
            php_chunks.iter().any(|chunk| {
                chunk
                    .metadata
                    .symbol
                    .as_deref()
                    .is_some_and(|name| name.contains("hello") || name.contains("Greeter"))
            }),
            "PHP class/method symbols should be extracted"
        );
    }

    #[test]
    fn an_oversized_uncovered_block_is_split() {
        let mut code = String::from("fn tiny() {}\n\n");
        for index in 0..400 {
            code.push_str(&format!(
                "const VALUE_{index}: &str = \"{}\";\n",
                "x".repeat(40)
            ));
        }
        let chunks = chunk_code(&code, CodeLanguage::Rust, 80);
        assert!(chunks.len() > 1);
        let maximum_characters = 80usize.saturating_mul(6).max(512);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.text.chars().count() <= maximum_characters + 80),
            "every chunk should stay near the embed-safe size"
        );
    }

    #[test]
    fn smart_chunk_uses_the_code_chunker_for_a_rust_extension() {
        let code = r#"
fn hello() {
    println!("Hello");
}

fn world() {
    println!("World");
}
"#;
        let chunks = smart_chunk(code, "text/plain", Some("rs"), Some("src/lib.rs"), 512, 50);

        assert!(!chunks.is_empty());
    }

    #[test]
    fn smart_chunk_uses_the_code_chunker_for_a_python_extension() {
        let code = r#"
def hello():
    print("Hello")

def world():
    print("World")
"#;
        let chunks = smart_chunk(code, "text/plain", Some("py"), Some("app.py"), 512, 50);

        assert!(!chunks.is_empty());
    }

    #[test]
    fn smart_chunk_detects_the_language_from_the_content_type() {
        let code = r#"
function hello() {
    console.log("Hello");
}
"#;
        let chunks = smart_chunk(code, "text/javascript", None, Some("index.js"), 512, 50);

        assert!(!chunks.is_empty());
    }

    #[test]
    fn smart_chunk_falls_back_to_text_for_plain_text() {
        let text = "This is plain text. It should use text-based chunking.";
        let chunks = smart_chunk(text, "text/plain", None, None, 512, 50);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, text);
    }

    #[test]
    fn smart_chunk_falls_back_to_text_for_an_unknown_extension() {
        let text = "Some content with unknown extension.";
        let chunks = smart_chunk(text, "text/plain", Some("xyz"), None, 512, 50);

        assert_eq!(chunks.len(), 1);
    }
}
