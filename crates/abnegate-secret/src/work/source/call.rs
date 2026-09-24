//! Calls in a masked scanner statement, as the walk rule reads them.

use crate::work::source::expression::Expression;
use crate::work::source::expression::closing;
use crate::work::source::expression::identifier_start;
use crate::work::source::expression::identifiers;
use crate::work::source::expression::opening;

/// Words a bracket follows without calling them.
const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

/// A call in a statement: of a method, of a function by its path or its name,
/// or of a macro.
pub(super) struct Call<'a> {
    /// What is called.
    pub(super) name: &'a str,
    /// The path the function is named through, as `String::` or
    /// `crate::work::`: empty for a method, and for a function named alone.
    pub(super) path: &'a str,
    /// What the method is called on, and `None` for anything but a method.
    pub(super) receiver: Option<Expression<'a>>,
    /// What is between its brackets.
    pub(super) arguments: &'a str,
    /// Whether it expands a macro rather than calling a function.
    pub(super) expands: bool,
}

impl<'a> Call<'a> {
    /// Every call in `code`, but for the declarations of functions.
    pub(super) fn every(code: &'a str) -> Vec<Self> {
        identifiers(code)
            .filter_map(|(start, name)| Self::named(code, start, name))
            .collect()
    }

    /// The call of `name`, which starts at `start` in `code`, if it is called.
    fn named(code: &'a str, start: usize, name: &'a str) -> Option<Self> {
        let bytes = code.as_bytes();
        let before = &code[..start];
        let previous = before.trim_end();
        if KEYWORDS.contains(&name)
            || &previous[identifier_start(previous.as_bytes(), previous.len())..] == "fn"
        {
            return None;
        }
        let mut open = start + name.len();
        if code[open..].starts_with("::<") {
            open = (closing(bytes, open + 2) + 1).min(code.len());
        }
        let expands = code.get(open..).is_some_and(|rest| rest.starts_with('!'))
            && matches!(bytes.get(open + 1), Some(b'(' | b'[' | b'{'));
        if expands {
            open += 1;
        } else if bytes.get(open) != Some(&b'(') {
            return None;
        }
        let arguments = code.get(open + 1..closing(bytes, open)).unwrap_or_default();
        let method = before.ends_with('.') && !before.ends_with("..");
        let path = if before.ends_with("::") {
            &code[path_start(code, start)..start]
        } else {
            ""
        };
        Some(Self {
            name,
            path,
            receiver: method.then(|| Expression::before(code, start - 1)),
            arguments,
            expands,
        })
    }
}

impl Call<'_> {
    /// The path the function is named through and its name, without generic
    /// arguments: `Vec::new` for `Vec::<u8>::new`.
    pub(super) fn qualified(&self) -> String {
        let mut path = self.path.to_owned();
        while let Some(open) = path.find("::<") {
            let close = closing(path.as_bytes(), open + 2);
            path.replace_range(open..(close + 1).min(path.len()), "");
        }
        format!("{path}{}", self.name)
    }

    /// The last name on the path the function is named through, as `Vec` for
    /// `Vec::new`: its type or module.
    pub(super) fn owner(&self) -> String {
        let qualified = self.qualified();
        let mut segments = qualified.rsplit("::");
        segments.next();
        segments.next().unwrap_or_default().to_owned()
    }
}

/// Where the path before the name at `start` starts, over its names and any
/// generic arguments or qualified type in it.
fn path_start(code: &str, start: usize) -> usize {
    let bytes = code.as_bytes();
    let mut start = start;
    while code[..start].ends_with("::") {
        start -= 2;
        start = if code[..start].ends_with('>') {
            opening(bytes, start - 1)
        } else {
            identifier_start(bytes, start)
        };
    }
    start
}
