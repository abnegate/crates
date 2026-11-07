//! Parse source with tree-sitter and read symbols and file context from the
//! tree.

use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;
use tree_sitter::Parser;
use tree_sitter::Tree;

use crate::code::file_context::FileContext;
use crate::code::language::CodeLanguage;
use crate::code::symbol::ParsedSymbol;
use crate::code::visibility::Visibility;

const MAXIMUM_PARSE_SIZE: usize = 10 * 1024 * 1024;

const NAME_FIELDS: &[&str] = &["name", "declarator", "identifier"];

static SQL_CREATE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)CREATE\s+(?:OR\s+REPLACE\s+)?(?:TABLE|FUNCTION|VIEW|INDEX)\s+(?:IF\s+NOT\s+EXISTS\s+)?(\w+)")
        .expect("the SQL create pattern is a valid regex")
});

pub(super) fn parse_code(content: &str, language: CodeLanguage) -> Option<Tree> {
    if content.len() > MAXIMUM_PARSE_SIZE {
        return None;
    }

    let grammar = language.tree_sitter_language()?;
    let mut parser = Parser::new();
    parser.set_language(&grammar).ok()?;
    parser.parse(content, None)
}

pub(super) fn extract_file_context(
    root: &Node<'_>,
    source: &[u8],
    language: CodeLanguage,
) -> FileContext {
    let mut context = FileContext::default();
    let import_types = language.import_node_types();
    let comment_types = language.comment_node_types();

    let mut cursor = root.walk();
    let mut imports = Vec::new();
    let mut first_import_start: Option<usize> = None;
    let mut last_import_end = 0;

    if cursor.goto_first_child() {
        loop {
            let node = cursor.node();
            let kind = node.kind();

            if comment_types.contains(&kind)
                && context.file_docstring.is_none()
                && let Ok(text) = node.utf8_text(source)
            {
                context.file_docstring = Some(text.to_string());
            }

            if (kind.contains("module")
                || kind.contains("package")
                || kind == "mod_item"
                || kind == "package_header")
                && let Ok(text) = node.utf8_text(source)
            {
                context.module_declaration = Some(text.to_string());
            }

            if import_types.contains(&kind) {
                if first_import_start.is_none() {
                    first_import_start = Some(node.start_byte());
                }
                last_import_end = node.end_byte();
                if let Ok(text) = node.utf8_text(source) {
                    imports.push(text.to_string());
                }
            }

            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }

    context.imports = imports.join("\n");
    context.import_start = first_import_start.unwrap_or(0);
    context.import_end = last_import_end;

    context
}

pub(super) fn extract_symbols(
    node: &Node<'_>,
    source: &[u8],
    language: CodeLanguage,
    parent_names: Vec<String>,
) -> Vec<ParsedSymbol> {
    let semantic_types = language.semantic_node_types();
    let comment_types = language.comment_node_types();
    let visibility_types = language.visibility_node_types();

    let mut symbols = Vec::new();
    let mut cursor = node.walk();

    if !cursor.goto_first_child() {
        return symbols;
    }

    let mut previous_comment: Option<String> = None;

    loop {
        let current = cursor.node();
        let kind = current.kind();

        if comment_types.contains(&kind) {
            if let Ok(text) = current.utf8_text(source) {
                previous_comment = Some(text.to_string());
            }
        } else if semantic_types.contains(&kind) {
            let name = extract_name(&current, source, language);
            let visibility = extract_visibility(&current, source, language, visibility_types);
            let signature = extract_signature(&current, source, language);
            let body = current
                .utf8_text(source)
                .map(|text| text.to_string())
                .unwrap_or_default();

            let is_public = matches!(visibility, Visibility::Public)
                || (language == CodeLanguage::Go
                    && name.as_ref().is_some_and(|name| is_exported_go(name)));

            let mut symbol = ParsedSymbol {
                node_type: kind.to_string(),
                name: name.clone(),
                start_byte: current.start_byte(),
                end_byte: current.end_byte(),
                visibility,
                signature,
                docstring: previous_comment.take(),
                body,
                parent_names: parent_names.clone(),
                nested_symbols: Vec::new(),
                is_public,
            };

            let mut nested_parents = parent_names.clone();
            if let Some(ref name) = name {
                nested_parents.push(name.clone());
            }
            symbol.nested_symbols = extract_symbols(&current, source, language, nested_parents);

            symbols.push(symbol);
        } else {
            symbols.extend(extract_symbols(
                &current,
                source,
                language,
                parent_names.clone(),
            ));
            previous_comment = None;
        }

        if !cursor.goto_next_sibling() {
            break;
        }
    }

    symbols
}

fn extract_name(node: &Node<'_>, source: &[u8], language: CodeLanguage) -> Option<String> {
    if language == CodeLanguage::Rust
        && node.kind() == "impl_item"
        && let Some(type_node) = node.child_by_field_name("type")
        && let Ok(text) = type_node.utf8_text(source)
    {
        return Some(text.to_string());
    }

    for field in NAME_FIELDS {
        if let Some(name_node) = node.child_by_field_name(field) {
            let name_node = if name_node.kind() == "function_declarator" {
                name_node
                    .child_by_field_name("declarator")
                    .unwrap_or(name_node)
            } else {
                name_node
            };

            if let Ok(text) = name_node.utf8_text(source) {
                return Some(text.to_string());
            }
        }
    }

    match language {
        CodeLanguage::Sql => {
            let text = node.utf8_text(source).ok()?;
            SQL_CREATE_NAME
                .captures(text)
                .and_then(|captures| captures.get(1))
                .map(|matched| matched.as_str().to_string())
        }
        _ => None,
    }
}

fn extract_visibility(
    node: &Node<'_>,
    source: &[u8],
    language: CodeLanguage,
    visibility_types: &[&str],
) -> Visibility {
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if visibility_types.contains(&child.kind())
                && let Ok(text) = child.utf8_text(source)
            {
                let text = text.to_lowercase();
                if text.contains("pub") || text.contains("public") || text.contains("export") {
                    return Visibility::Public;
                }
                if text.contains("private") {
                    return Visibility::Private;
                }
                if text.contains("protected") {
                    return Visibility::Protected;
                }
                if text.contains("internal") {
                    return Visibility::Internal;
                }
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }

    if language == CodeLanguage::Go
        && let Some(name) = extract_name(node, source, language)
        && is_exported_go(&name)
    {
        return Visibility::Public;
    }

    Visibility::Private
}

fn is_exported_go(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|character| character.is_uppercase())
}

fn extract_signature(node: &Node<'_>, source: &[u8], language: CodeLanguage) -> String {
    let full_text = node.utf8_text(source).unwrap_or("");

    match language {
        CodeLanguage::Rust => {
            if let Some(brace_position) = full_text.find('{') {
                return full_text[..brace_position].trim().to_string();
            }
            full_text.lines().next().unwrap_or("").to_string()
        }
        CodeLanguage::Python => {
            if let Some(colon_position) = full_text.find(':') {
                return full_text[..=colon_position].trim().to_string();
            }
            full_text.lines().next().unwrap_or("").to_string()
        }
        CodeLanguage::JavaScript | CodeLanguage::TypeScript | CodeLanguage::Tsx => {
            if let Some(brace_position) = full_text.find('{') {
                return full_text[..brace_position].trim().to_string();
            }
            full_text.lines().next().unwrap_or("").to_string()
        }
        CodeLanguage::Go => {
            if let Some(brace_position) = full_text.find('{') {
                return full_text[..brace_position].trim().to_string();
            }
            full_text.lines().next().unwrap_or("").to_string()
        }
        CodeLanguage::Java | CodeLanguage::Kotlin => {
            if let Some(brace_position) = full_text.find('{') {
                return full_text[..brace_position].trim().to_string();
            }
            full_text.lines().next().unwrap_or("").to_string()
        }
        CodeLanguage::Swift => {
            if let Some(brace_position) = full_text.find('{') {
                return full_text[..brace_position].trim().to_string();
            }
            full_text.lines().next().unwrap_or("").to_string()
        }
        CodeLanguage::C | CodeLanguage::Cpp => {
            if let Some(brace_position) = full_text.find('{') {
                return full_text[..brace_position].trim().to_string();
            }
            full_text.lines().next().unwrap_or("").to_string()
        }
        _ => full_text.lines().next().unwrap_or("").to_string(),
    }
}
