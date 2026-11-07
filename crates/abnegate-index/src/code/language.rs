//! The languages the code chunker parses, and the syntax node types it reads
//! for each.

use tree_sitter::Language;

/// A language the code chunker can parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CodeLanguage {
    /// Rust.
    Rust,
    /// Python.
    Python,
    /// JavaScript, including JSX.
    JavaScript,
    /// TypeScript.
    TypeScript,
    /// TypeScript with JSX.
    Tsx,
    /// Go.
    Go,
    /// Java.
    Java,
    /// C.
    C,
    /// C++.
    Cpp,
    /// Kotlin.
    Kotlin,
    /// Swift.
    Swift,
    /// SQL.
    Sql,
    /// JSON.
    Json,
    /// YAML.
    Yaml,
    /// Ruby.
    Ruby,
    /// PHP.
    Php,
    /// No language the chunker can parse; content falls back to text windows.
    Unknown,
}

impl CodeLanguage {
    /// The language a file `extension` names, matched case-insensitively and
    /// without the leading dot.
    pub fn from_extension(extension: &str) -> Self {
        match extension.to_lowercase().as_str() {
            "rs" => Self::Rust,
            "py" | "pyw" | "pyi" => Self::Python,
            "js" | "mjs" | "cjs" | "jsx" => Self::JavaScript,
            "ts" | "mts" | "cts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "go" => Self::Go,
            "java" => Self::Java,
            "c" | "h" => Self::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => Self::Cpp,
            "kt" | "kts" => Self::Kotlin,
            "swift" => Self::Swift,
            "sql" => Self::Sql,
            "json" => Self::Json,
            "yaml" | "yml" => Self::Yaml,
            "rb" => Self::Ruby,
            "php" => Self::Php,
            _ => Self::Unknown,
        }
    }

    /// The language a content type such as `text/x-rust` names, matched by
    /// substring and case-insensitively.
    pub fn from_content_type(content_type: &str) -> Self {
        let lowered = content_type.to_lowercase();
        if lowered.contains("rust") {
            Self::Rust
        } else if lowered.contains("python") {
            Self::Python
        } else if lowered.contains("javascript") {
            Self::JavaScript
        } else if lowered.contains("tsx") {
            Self::Tsx
        } else if lowered.contains("typescript") {
            Self::TypeScript
        } else if lowered.contains("golang") || lowered.contains("/go") {
            Self::Go
        } else if lowered.contains("java") && !lowered.contains("javascript") {
            Self::Java
        } else if lowered.contains("kotlin") {
            Self::Kotlin
        } else if lowered.contains("swift") {
            Self::Swift
        } else if lowered.contains("sql") {
            Self::Sql
        } else if lowered.contains("c++") || lowered.contains("cpp") {
            Self::Cpp
        } else if lowered.contains("/c") || lowered.contains("x-c") {
            Self::C
        } else if lowered.contains("json") {
            Self::Json
        } else if lowered.contains("yaml") {
            Self::Yaml
        } else if lowered.contains("ruby") {
            Self::Ruby
        } else if lowered.contains("php") {
            Self::Php
        } else {
            Self::Unknown
        }
    }

    pub(super) fn tree_sitter_language(&self) -> Option<Language> {
        match self {
            Self::Rust => Some(tree_sitter_rust::LANGUAGE.into()),
            Self::Python => Some(tree_sitter_python::LANGUAGE.into()),
            Self::JavaScript => Some(tree_sitter_javascript::LANGUAGE.into()),
            Self::TypeScript => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
            Self::Tsx => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
            Self::Ruby => Some(tree_sitter_ruby::LANGUAGE.into()),
            Self::Php => Some(tree_sitter_php::LANGUAGE_PHP.into()),
            Self::Go => Some(tree_sitter_go::LANGUAGE.into()),
            Self::Java => Some(tree_sitter_java::LANGUAGE.into()),
            Self::C => Some(tree_sitter_c::LANGUAGE.into()),
            Self::Cpp => Some(tree_sitter_cpp::LANGUAGE.into()),
            Self::Kotlin => Some(tree_sitter_kotlin_sg::LANGUAGE.into()),
            Self::Swift => Some(tree_sitter_swift::LANGUAGE.into()),
            Self::Sql => Some(tree_sitter_sequel::LANGUAGE.into()),
            Self::Json => Some(tree_sitter_json::LANGUAGE.into()),
            Self::Yaml => Some(tree_sitter_yaml::LANGUAGE.into()),
            Self::Unknown => None,
        }
    }

    /// The stable lowercase label written into chunk headers, such as `rust`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::Go => "go",
            Self::Java => "java",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Kotlin => "kotlin",
            Self::Swift => "swift",
            Self::Sql => "sql",
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::Ruby => "ruby",
            Self::Php => "php",
            Self::Unknown => "unknown",
        }
    }

    pub(super) fn is_container_node(self, node_type: &str) -> bool {
        match self {
            Self::Rust => matches!(node_type, "impl_item" | "mod_item" | "trait_item"),
            Self::Python => matches!(node_type, "class_definition"),
            Self::JavaScript | Self::TypeScript | Self::Tsx => {
                matches!(node_type, "class_declaration")
            }
            Self::Java | Self::Kotlin | Self::Php => matches!(node_type, "class_declaration"),
            Self::Cpp => matches!(node_type, "class_specifier" | "namespace_definition"),
            Self::Ruby => matches!(node_type, "class" | "module"),
            Self::Swift => matches!(
                node_type,
                "class_declaration" | "protocol_declaration" | "extension_declaration"
            ),
            _ => false,
        }
    }

    pub(super) fn semantic_node_types(&self) -> &'static [&'static str] {
        match self {
            Self::Rust => &[
                "function_item",
                "impl_item",
                "struct_item",
                "enum_item",
                "mod_item",
                "trait_item",
                "const_item",
                "static_item",
                "type_item",
                "macro_definition",
            ],
            Self::Python => &[
                "function_definition",
                "class_definition",
                "decorated_definition",
            ],
            Self::JavaScript | Self::TypeScript | Self::Tsx => &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "arrow_function",
                "variable_declaration",
                "lexical_declaration",
                "export_statement",
                "interface_declaration",
                "type_alias_declaration",
            ],
            Self::Go => &[
                "function_declaration",
                "method_declaration",
                "type_declaration",
                "const_declaration",
                "var_declaration",
            ],
            Self::Java => &[
                "class_declaration",
                "method_declaration",
                "interface_declaration",
                "constructor_declaration",
                "enum_declaration",
                "annotation_type_declaration",
            ],
            Self::C | Self::Cpp => &[
                "function_definition",
                "struct_specifier",
                "class_specifier",
                "namespace_definition",
                "enum_specifier",
                "template_declaration",
            ],
            Self::Kotlin => &[
                "function_declaration",
                "class_declaration",
                "object_declaration",
                "companion_object",
                "property_declaration",
            ],
            Self::Swift => &[
                "function_declaration",
                "class_declaration",
                "struct_declaration",
                "enum_declaration",
                "protocol_declaration",
                "extension_declaration",
            ],
            Self::Sql => &[
                "create_table_statement",
                "create_function_statement",
                "create_view_statement",
                "create_index_statement",
                "select_statement",
                "insert_statement",
                "update_statement",
                "delete_statement",
            ],
            Self::Json => &["object", "array"],
            Self::Yaml => &["block_mapping", "block_sequence"],
            Self::Ruby => &["method", "singleton_method", "class", "module"],
            Self::Php => &[
                "function_definition",
                "method_declaration",
                "class_declaration",
                "interface_declaration",
                "trait_declaration",
                "enum_declaration",
            ],
            Self::Unknown => &[],
        }
    }

    pub(super) fn import_node_types(&self) -> &'static [&'static str] {
        match self {
            Self::Rust => &["use_declaration", "extern_crate_declaration"],
            Self::Python => &["import_statement", "import_from_statement"],
            Self::JavaScript | Self::TypeScript | Self::Tsx => {
                &["import_statement", "import_declaration"]
            }
            Self::Go => &["import_declaration", "import_spec"],
            Self::Java => &["import_declaration", "package_declaration"],
            Self::C | Self::Cpp => &["preproc_include", "preproc_import"],
            Self::Kotlin => &["import_header", "package_header"],
            Self::Swift => &["import_declaration"],
            Self::Sql => &[],
            Self::Json | Self::Yaml => &[],
            Self::Ruby => &["identifier"],
            Self::Php => &["namespace_use_declaration", "namespace_definition"],
            Self::Unknown => &[],
        }
    }

    pub(super) fn comment_node_types(&self) -> &'static [&'static str] {
        match self {
            Self::Rust => &["line_comment", "block_comment", "doc_comment"],
            Self::Python => &["comment", "string"],
            Self::JavaScript | Self::TypeScript | Self::Tsx => &["comment", "block_comment"],
            Self::Go => &["comment"],
            Self::Java => &["line_comment", "block_comment"],
            Self::C | Self::Cpp => &["comment"],
            Self::Kotlin => &["line_comment", "multiline_comment"],
            Self::Swift => &["comment", "multiline_comment"],
            Self::Sql => &["comment", "marginalia"],
            _ => &[],
        }
    }

    pub(super) fn visibility_node_types(&self) -> &'static [&'static str] {
        match self {
            Self::Rust => &["visibility_modifier"],
            Self::Python => &[],
            Self::JavaScript | Self::TypeScript | Self::Tsx => {
                &["accessibility_modifier", "export"]
            }
            Self::Go => &[],
            Self::Java => &["modifiers"],
            Self::C | Self::Cpp => &["storage_class_specifier", "type_qualifier"],
            Self::Kotlin => &["visibility_modifier", "modifiers"],
            Self::Swift => &["access_level_modifier"],
            _ => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_to_their_languages() {
        assert_eq!(CodeLanguage::from_extension("rs"), CodeLanguage::Rust);
        assert_eq!(CodeLanguage::from_extension("py"), CodeLanguage::Python);
        assert_eq!(CodeLanguage::from_extension("js"), CodeLanguage::JavaScript);
        assert_eq!(CodeLanguage::from_extension("ts"), CodeLanguage::TypeScript);
        assert_eq!(CodeLanguage::from_extension("tsx"), CodeLanguage::Tsx);
        assert_eq!(CodeLanguage::from_extension("rb"), CodeLanguage::Ruby);
        assert_eq!(CodeLanguage::from_extension("php"), CodeLanguage::Php);
        assert_eq!(CodeLanguage::from_extension("go"), CodeLanguage::Go);
        assert_eq!(CodeLanguage::from_extension("java"), CodeLanguage::Java);
        assert_eq!(CodeLanguage::from_extension("kt"), CodeLanguage::Kotlin);
        assert_eq!(CodeLanguage::from_extension("swift"), CodeLanguage::Swift);
        assert_eq!(CodeLanguage::from_extension("sql"), CodeLanguage::Sql);
        assert_eq!(
            CodeLanguage::from_extension("unknown"),
            CodeLanguage::Unknown
        );
    }

    #[test]
    fn content_types_map_to_their_languages() {
        assert_eq!(
            CodeLanguage::from_content_type("text/x-rust"),
            CodeLanguage::Rust
        );
        assert_eq!(
            CodeLanguage::from_content_type("text/x-kotlin"),
            CodeLanguage::Kotlin
        );
        assert_eq!(
            CodeLanguage::from_content_type("text/x-swift"),
            CodeLanguage::Swift
        );
        assert_eq!(
            CodeLanguage::from_content_type("text/plain"),
            CodeLanguage::Unknown
        );
    }
}
