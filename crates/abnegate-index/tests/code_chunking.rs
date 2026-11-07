use std::collections::HashSet;

use abnegate_index::CodeChunkType;
use abnegate_index::CodeLanguage;
use abnegate_index::SymbolKind;
use abnegate_index::chunk_code;
use abnegate_index::smart_chunk;

#[test]
fn rust_functions_become_header_body_and_api_surface_chunks() {
    let code = r#"
use std::io;

/// Documentation for hello - a longer function with more logic
/// that should exceed the min_tokens threshold
pub fn hello(name: &str, greeting: &str) -> String {
    let mut result = String::new();
    result.push_str(greeting);
    result.push_str(", ");
    result.push_str(name);
    result.push('!');
    println!("Generated greeting: {}", result);
    result
}

/// Documentation for goodbye - another function with logic
/// that should exceed the min_tokens threshold for chunking
pub fn goodbye(name: &str, farewell: &str) -> String {
    let mut result = String::new();
    result.push_str(farewell);
    result.push_str(", ");
    result.push_str(name);
    result.push('!');
    println!("Generated farewell: {}", result);
    result
}

pub struct Point {
    x: i32,
    y: i32,
}

impl Point {
    pub fn new(x: i32, y: i32) -> Self {
        Point { x, y }
    }

    fn distance(&self) -> f64 {
        ((self.x.pow(2) + self.y.pow(2)) as f64).sqrt()
    }
}
"#;

    let chunks = chunk_code(code, CodeLanguage::Rust, 512);

    assert!(!chunks.is_empty(), "Expected chunks, got none");

    let has_header = chunks
        .iter()
        .any(|chunk| chunk.chunk_type == CodeChunkType::FileHeader);
    assert!(has_header, "Should have file header chunk");

    let body_chunks: Vec<_> = chunks
        .iter()
        .filter(|chunk| chunk.chunk_type == CodeChunkType::Body)
        .collect();
    assert!(!body_chunks.is_empty(), "Should have body chunks");

    let has_hello = body_chunks
        .iter()
        .any(|chunk| chunk.text.contains("fn hello"));
    let has_goodbye = body_chunks
        .iter()
        .any(|chunk| chunk.text.contains("fn goodbye"));
    assert!(
        has_hello || has_goodbye,
        "Should have function content in body chunks"
    );

    let has_struct = body_chunks
        .iter()
        .any(|chunk| chunk.text.contains("struct Point"));
    assert!(has_struct, "Should have struct content");

    let has_api_surface = chunks
        .iter()
        .any(|chunk| chunk.chunk_type == CodeChunkType::ApiSurface);
    assert!(
        has_api_surface,
        "Should have API surface chunk (public symbols)"
    );
}

#[test]
fn python_classes_and_functions_become_body_chunks() {
    let code = r#"
import os
from typing import List

def standalone_function():
    """A standalone function that does something useful and interesting"""
    result = 42
    for i in range(10):
        result += i
    return result

class Calculator:
    """A simple calculator class with multiple methods for arithmetic operations"""

    def __init__(self):
        self.result = 0
        self.history = []

    def add(self, x, y):
        """Add two numbers and store in history"""
        self.result = x + y
        self.history.append(('add', x, y, self.result))
        return self.result

    def subtract(self, x, y):
        """Subtract two numbers"""
        self.result = x - y
        return self.result

class AdvancedCalculator(Calculator):
    """An advanced calculator with more operations"""

    def multiply(self, x, y):
        self.result = x * y
        return self.result
"#;

    let chunks = chunk_code(code, CodeLanguage::Python, 512);

    assert!(!chunks.is_empty(), "Expected chunks, got none");

    let body_chunks: Vec<_> = chunks
        .iter()
        .filter(|chunk| chunk.chunk_type == CodeChunkType::Body)
        .collect();
    assert!(!body_chunks.is_empty(), "Should have body chunks");

    let function_chunks: Vec<_> = chunks
        .iter()
        .filter(|chunk| matches!(chunk.metadata.kind, Some(SymbolKind::Function)))
        .collect();
    assert!(!function_chunks.is_empty(), "Should have function chunk");

    let class_chunks: Vec<_> = chunks
        .iter()
        .filter(|chunk| matches!(chunk.metadata.kind, Some(SymbolKind::Class)))
        .collect();
    assert!(!class_chunks.is_empty(), "Should have class chunks");
}

#[test]
fn javascript_functions_and_classes_are_chunked() {
    let code = r#"
function regularFunction() {
    console.log("Regular function");
}

const arrowFunction = () => {
    console.log("Arrow function");
};

class MyClass {
    constructor(name) {
        this.name = name;
    }

    greet() {
        console.log(`Hello, ${this.name}`);
    }

    static create(name) {
        return new MyClass(name);
    }
}

export default MyClass;
"#;

    let chunks = chunk_code(code, CodeLanguage::JavaScript, 512);

    assert!(!chunks.is_empty(), "Expected chunks, got none");
}

#[test]
fn typescript_interfaces_and_classes_are_chunked() {
    let code = r#"
interface User {
    id: number;
    name: string;
}

function getUser(id: number): User {
    return { id, name: "Test User" };
}

class UserService {
    private users: User[] = [];

    addUser(user: User): void {
        this.users.push(user);
    }

    getUsers(): User[] {
        return this.users;
    }
}
"#;

    let chunks = chunk_code(code, CodeLanguage::TypeScript, 512);

    assert!(!chunks.is_empty(), "Should have at least one chunk");
}

#[test]
fn go_types_and_methods_are_chunked() {
    let code = r#"
package main

import "fmt"

type Point struct {
    X int
    Y int
}

func (p Point) String() string {
    return fmt.Sprintf("(%d, %d)", p.X, p.Y)
}

func main() {
    p := Point{X: 10, Y: 20}
    fmt.Println(p)
}
"#;

    let chunks = chunk_code(code, CodeLanguage::Go, 512);

    assert!(!chunks.is_empty(), "Should have at least one chunk");
}

#[test]
fn java_classes_and_interfaces_are_chunked() {
    let code = r#"
public class Calculator {
    private int result;

    public Calculator() {
        this.result = 0;
    }

    public int add(int a, int b) {
        this.result = a + b;
        return this.result;
    }

    public int getResult() {
        return this.result;
    }
}

interface Printable {
    void print();
}
"#;

    let chunks = chunk_code(code, CodeLanguage::Java, 512);

    assert!(!chunks.is_empty(), "Expected chunks, got none");
}

#[test]
fn kotlin_data_classes_and_functions_are_chunked() {
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
    assert!(!chunks.is_empty(), "Should chunk Kotlin code");
}

#[test]
fn swift_structs_and_functions_are_chunked() {
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
    assert!(!chunks.is_empty(), "Should chunk Swift code");
}

#[test]
fn sql_statements_are_chunked() {
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
    assert!(!chunks.is_empty(), "Should chunk SQL code");
}

#[test]
fn c_structs_and_functions_are_chunked() {
    let code = r#"
#include <stdio.h>

struct Point {
    int x;
    int y;
};

void print_point(struct Point p) {
    printf("(%d, %d)\n", p.x, p.y);
}

int main() {
    struct Point p = {10, 20};
    print_point(p);
    return 0;
}
"#;

    let chunks = chunk_code(code, CodeLanguage::C, 512);

    assert!(!chunks.is_empty(), "Should have at least one chunk");
}

#[test]
fn a_function_over_the_token_limit_splits_into_non_empty_chunks() {
    let mut code = String::from("fn large_function() {\n");
    for index in 0..500 {
        code.push_str(&format!("    let var_{index} = {index};\n"));
    }
    code.push_str("}\n");

    let chunks = chunk_code(&code, CodeLanguage::Rust, 100);

    assert!(
        chunks.len() > 1,
        "Large function should be split into multiple chunks"
    );

    for chunk in &chunks {
        assert!(!chunk.text.trim().is_empty(), "Chunk should not be empty");
    }
}

#[test]
fn an_empty_file_produces_no_chunks() {
    let chunks = chunk_code("", CodeLanguage::Rust, 512);
    assert!(chunks.is_empty(), "Empty file should produce no chunks");
}

#[test]
fn a_syntax_error_falls_back_to_top_level_text_chunks() {
    let bad_code = r#"
fn incomplete_function(
    // Missing closing parenthesis and body
"#;

    let chunks = chunk_code(bad_code, CodeLanguage::Rust, 512);

    assert!(!chunks.is_empty(), "Should fall back to text chunking");
    assert!(
        chunks
            .iter()
            .any(|chunk| chunk.chunk_type == CodeChunkType::TopLevel),
        "Fallback chunks should be TopLevel"
    );
}

#[test]
fn a_comments_only_file_still_produces_chunks() {
    let code = r#"
// This is a comment
// Another comment
/*
 * Multi-line comment
 */
"#;

    let chunks = chunk_code(code, CodeLanguage::Rust, 512);

    assert!(!chunks.is_empty(), "Should handle comment-only files");
}

#[test]
fn smart_chunk_detects_code_by_extension_or_content_type_and_falls_back_to_text() {
    let rust_code = r#"
pub fn hello() {
    println!("Hello");
}

pub fn world() {
    println!("World");
}
"#;

    let chunks = smart_chunk(
        rust_code,
        "text/plain",
        Some("rs"),
        Some("src/lib.rs"),
        512,
        50,
    );
    assert!(!chunks.is_empty(), "Should detect Rust and produce chunks");

    let chunks = smart_chunk(rust_code, "text/x-rust", None, Some("src/lib.rs"), 512, 50);
    assert!(!chunks.is_empty(), "Should detect Rust from content type");

    let plain_text = "This is just plain text without code structure.";
    let chunks = smart_chunk(plain_text, "text/plain", None, None, 512, 50);
    assert_eq!(chunks.len(), 1, "Plain text should use text chunking");
}

#[test]
fn functions_with_mixed_comments_become_body_chunks() {
    let code = r#"
// Top-level comment

/// Documentation for the function
pub fn documented_function() {
    // Inline comment
    let x = 42;
    /* Block comment */
    println!("{}", x);
}

// Another comment

pub fn another_function() {
    // More comments
    let y = 100;
    println!("{}", y);
}
"#;

    let chunks = chunk_code(code, CodeLanguage::Rust, 512);

    let body_chunks: Vec<_> = chunks
        .iter()
        .filter(|chunk| chunk.chunk_type == CodeChunkType::Body)
        .collect();
    assert!(!body_chunks.is_empty(), "Should have body chunks");
}

#[test]
fn methods_of_an_impl_nested_in_a_module_are_indexed() {
    let code = r#"
mod outer {
    pub struct Outer {
        inner: Inner,
    }

    struct Inner {
        value: i32,
    }

    impl Outer {
        pub fn new() -> Self {
            Outer {
                inner: Inner { value: 0 }
            }
        }
    }
}
"#;

    let chunks = chunk_code(code, CodeLanguage::Rust, 512);

    assert!(!chunks.is_empty(), "Should handle nested structures");
    let symbols: Vec<_> = chunks
        .iter()
        .filter_map(|chunk| chunk.metadata.symbol.clone())
        .collect();
    assert!(
        symbols.iter().any(|name| name.contains("new")),
        "nested impl methods should be indexed, got {symbols:?}"
    );
}

#[test]
fn json_documents_are_chunked() {
    let json = r#"
{
    "users": [
        {
            "id": 1,
            "name": "Alice"
        },
        {
            "id": 2,
            "name": "Bob"
        }
    ],
    "settings": {
        "theme": "dark",
        "notifications": true
    }
}
"#;

    let chunks = chunk_code(json, CodeLanguage::Json, 512);

    assert!(!chunks.is_empty(), "Should chunk JSON");
}

#[test]
fn body_chunks_follow_source_order() {
    let code = r#"
pub fn alpha() { println!("a"); }
pub fn beta() { println!("b"); }
pub fn gamma() { println!("c"); }
pub fn delta() { println!("d"); }
"#;

    let chunks = chunk_code(code, CodeLanguage::Rust, 512);

    let body_chunks: Vec<_> = chunks
        .iter()
        .filter(|chunk| chunk.chunk_type == CodeChunkType::Body)
        .collect();

    for pair in body_chunks.windows(2) {
        assert!(
            pair[1].start_offset >= pair[0].start_offset,
            "Body chunks should be ordered by position"
        );
    }
}

#[test]
fn unicode_text_survives_chunking() {
    let code = r#"
pub fn greet() {
    println!("Hello 世界!");
    println!("Здравствуй мир!");
    println!("مرحبا بالعالم!");
}
"#;

    let chunks = chunk_code(code, CodeLanguage::Rust, 512);

    assert!(!chunks.is_empty(), "Should handle Unicode in code");

    let has_unicode = chunks.iter().any(|chunk| chunk.text.contains("世界"));
    assert!(has_unicode, "Unicode characters should be preserved");
}

#[test]
fn a_large_enough_symbol_gets_body_and_skeleton_views() {
    let code = r#"
use std::collections::HashMap;

/// A public function with multiple parameters and complex logic
pub fn process_data(input: &str, options: HashMap<String, String>) -> Result<String, std::io::Error> {
    let mut result = String::new();
    for (key, value) in options.iter() {
        result.push_str(&format!("{}: {}\n", key, value));
    }
    result.push_str(input);
    Ok(result)
}
"#;

    let chunks = chunk_code(code, CodeLanguage::Rust, 512);

    let has_body = chunks
        .iter()
        .any(|chunk| chunk.chunk_type == CodeChunkType::Body);
    let has_skeleton = chunks
        .iter()
        .any(|chunk| chunk.chunk_type == CodeChunkType::Skeleton);

    assert!(has_body, "Should have body chunk");
    assert!(
        has_skeleton,
        "Should have skeleton chunk for sufficiently large symbol"
    );
}

#[test]
fn the_identifier_bag_names_the_defined_symbol_and_referenced_types() {
    let code = r#"
pub fn calculate_distance(point_a: Point, point_b: Point) -> f64 {
    let dx = point_b.x - point_a.x;
    let dy = point_b.y - point_a.y;
    sqrt(dx * dx + dy * dy)
}
"#;

    let chunks = chunk_code(code, CodeLanguage::Rust, 512);

    let chunk = chunks
        .iter()
        .find(|chunk| {
            chunk.chunk_type == CodeChunkType::Body && chunk.text.contains("calculate_distance")
        })
        .expect("Should have body chunk for function");

    assert_eq!(
        chunk.identifiers.defined_symbol,
        Some("calculate_distance".to_string())
    );
    assert!(
        chunk
            .identifiers
            .referenced_types
            .contains(&"Point".to_string())
    );
}

#[test]
fn generated_code_marks_every_chunk_as_generated() {
    let generated_code = r#"
// Code generated by some-generator. DO NOT EDIT.

fn generated_function() {
    println!("Generated");
}
"#;

    let chunks = chunk_code(generated_code, CodeLanguage::Rust, 512);

    assert!(!chunks.is_empty());
    let all_generated = chunks.iter().all(|chunk| chunk.metadata.is_generated);
    assert!(all_generated, "All chunks should be marked as generated");
}

#[test]
fn test_modules_are_detected_as_test_code() {
    let test_code = r#"
#[cfg(test)]
mod tests {
    #[test]
    fn test_something() {
        assert_eq!(1 + 1, 2);
    }
}
"#;

    let chunks = chunk_code(test_code, CodeLanguage::Rust, 512);

    let has_test = chunks.iter().any(|chunk| chunk.metadata.is_test);
    assert!(has_test, "Should detect test code");
}

#[test]
fn duplicate_definitions_produce_no_duplicate_content_hashes() {
    let code = r#"
fn same() {}
fn same() {}
"#;

    let chunks = chunk_code(code, CodeLanguage::Rust, 512);

    let mut hashes = HashSet::new();
    for chunk in &chunks {
        assert!(
            hashes.insert(chunk.metadata.content_hash.clone()),
            "Should not have duplicate content hashes"
        );
    }
}
