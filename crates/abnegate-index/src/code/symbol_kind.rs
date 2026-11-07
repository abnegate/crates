//! What a chunked symbol declares.

use crate::code::language::CodeLanguage;

/// What a chunked symbol declares. The `Debug` name is written into chunk
/// text as `kind: <name>`, so it never changes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SymbolKind {
    /// A free function.
    Function,
    /// A function inside a type, or nested in another symbol.
    Method,
    /// A class.
    Class,
    /// A struct.
    Struct,
    /// An enum.
    Enum,
    /// An interface or protocol.
    Interface,
    /// A trait.
    Trait,
    /// A Rust `impl` block.
    Impl,
    /// A module.
    Module,
    /// A constant.
    Constant,
    /// A variable, static or property.
    Variable,
    /// A type alias or type declaration.
    Type,
    /// An import.
    Import,
    /// A SQL statement.
    SqlStatement,
    /// Anything else, including merged groups of small symbols.
    Other,
}

impl SymbolKind {
    pub(super) fn from_node_type(node_type: &str, language: CodeLanguage) -> Self {
        match language {
            CodeLanguage::Rust => match node_type {
                "function_item" => Self::Function,
                "impl_item" => Self::Impl,
                "struct_item" => Self::Struct,
                "enum_item" => Self::Enum,
                "mod_item" => Self::Module,
                "trait_item" => Self::Trait,
                "const_item" => Self::Constant,
                "static_item" => Self::Variable,
                "type_item" => Self::Type,
                "use_declaration" => Self::Import,
                _ => Self::Other,
            },
            CodeLanguage::Python => match node_type {
                "function_definition" => Self::Function,
                "class_definition" => Self::Class,
                "import_statement" | "import_from_statement" => Self::Import,
                _ => Self::Other,
            },
            CodeLanguage::JavaScript | CodeLanguage::TypeScript | CodeLanguage::Tsx => {
                match node_type {
                    "function_declaration" | "arrow_function" => Self::Function,
                    "class_declaration" => Self::Class,
                    "method_definition" => Self::Method,
                    "interface_declaration" => Self::Interface,
                    "import_statement" => Self::Import,
                    _ => Self::Other,
                }
            }
            CodeLanguage::Ruby => match node_type {
                "method" | "singleton_method" => Self::Function,
                "class" => Self::Class,
                "module" => Self::Module,
                _ => Self::Other,
            },
            CodeLanguage::Php => match node_type {
                "function_definition" => Self::Function,
                "method_declaration" => Self::Method,
                "class_declaration" => Self::Class,
                "interface_declaration" => Self::Interface,
                "trait_declaration" => Self::Trait,
                "enum_declaration" => Self::Enum,
                _ => Self::Other,
            },
            CodeLanguage::Go => match node_type {
                "function_declaration" => Self::Function,
                "method_declaration" => Self::Method,
                "type_declaration" => Self::Type,
                "import_declaration" => Self::Import,
                _ => Self::Other,
            },
            CodeLanguage::Java => match node_type {
                "method_declaration" => Self::Method,
                "constructor_declaration" => Self::Method,
                "class_declaration" => Self::Class,
                "interface_declaration" => Self::Interface,
                "enum_declaration" => Self::Enum,
                "import_declaration" => Self::Import,
                _ => Self::Other,
            },
            CodeLanguage::Kotlin => match node_type {
                "function_declaration" => Self::Function,
                "class_declaration" => Self::Class,
                "object_declaration" => Self::Class,
                "property_declaration" => Self::Variable,
                "import_header" => Self::Import,
                _ => Self::Other,
            },
            CodeLanguage::Swift => match node_type {
                "function_declaration" => Self::Function,
                "class_declaration" => Self::Class,
                "struct_declaration" => Self::Struct,
                "enum_declaration" => Self::Enum,
                "protocol_declaration" => Self::Interface,
                "import_declaration" => Self::Import,
                _ => Self::Other,
            },
            CodeLanguage::Sql => match node_type {
                kind if kind.contains("create")
                    || kind.contains("select")
                    || kind.contains("insert") =>
                {
                    Self::SqlStatement
                }
                _ => Self::Other,
            },
            _ => Self::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_names_are_the_labels_written_into_chunk_text() {
        let names: Vec<String> = [
            SymbolKind::Function,
            SymbolKind::Method,
            SymbolKind::Class,
            SymbolKind::Struct,
            SymbolKind::Enum,
            SymbolKind::Interface,
            SymbolKind::Trait,
            SymbolKind::Impl,
            SymbolKind::Module,
            SymbolKind::Constant,
            SymbolKind::Variable,
            SymbolKind::Type,
            SymbolKind::Import,
            SymbolKind::SqlStatement,
            SymbolKind::Other,
        ]
        .iter()
        .map(|kind| format!("{kind:?}"))
        .collect();
        assert_eq!(
            names,
            [
                "Function",
                "Method",
                "Class",
                "Struct",
                "Enum",
                "Interface",
                "Trait",
                "Impl",
                "Module",
                "Constant",
                "Variable",
                "Type",
                "Import",
                "SqlStatement",
                "Other",
            ]
        );
    }
}
