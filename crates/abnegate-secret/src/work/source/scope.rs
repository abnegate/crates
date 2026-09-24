//! What the scanners declare, as the walk rule reads it.

use std::collections::BTreeSet;

use crate::work::source::expression::identifiers;
use crate::work::source::statement::Statement;

/// The module whose helpers count every byte they read.
pub(super) const WORK: &str = "work";

/// What the scanners declare, which a statement may name without leaving
/// what the rule reads.
pub(super) struct Scope {
    /// Every function the scanners declare, by name.
    pub(super) functions: BTreeSet<String>,
    /// Every closure the scanners bind to a name.
    pub(super) closures: BTreeSet<String>,
    /// Every type the scanners declare, by name.
    pub(super) types: BTreeSet<String>,
    /// Every module the rule reads, and `work`, by its path in the crate.
    pub(super) modules: BTreeSet<String>,
}

impl Scope {
    /// What `files`, each the path of its module in the crate and its
    /// statements, declare.
    pub(super) fn of<'a>(files: impl IntoIterator<Item = (&'a str, &'a [Statement])>) -> Self {
        let mut scope = Self {
            functions: BTreeSet::new(),
            closures: BTreeSet::new(),
            types: BTreeSet::new(),
            modules: BTreeSet::from([WORK.to_owned()]),
        };
        for (module, statements) in files {
            scope.modules.insert(module.to_owned());
            for statement in statements {
                scope.declare(module, &statement.code);
            }
        }
        scope
    }

    /// Add what `code`, a statement of `module`, declares.
    fn declare(&mut self, module: &str, code: &str) {
        let words: Vec<(usize, &str)> = identifiers(code).collect();
        for pair in words.windows(2) {
            let [(keyword_start, keyword), (start, name)] = pair else {
                continue;
            };
            if !code[keyword_start + keyword.len()..*start]
                .trim()
                .is_empty()
            {
                continue;
            }
            let after = code[start + name.len()..].trim_start();
            let name = (*name).to_owned();
            match *keyword {
                "fn" => {
                    self.functions.insert(name);
                }
                "enum" | "struct" | "trait" | "type" => {
                    self.types.insert(name);
                }
                "mod" if after.starts_with('{') => {
                    self.modules.insert(format!("{module}::{name}"));
                }
                "let" | "mut" if after.starts_with("= |") || after.starts_with("= move |") => {
                    self.closures.insert(name);
                }
                _ => {}
            }
        }
    }
}
