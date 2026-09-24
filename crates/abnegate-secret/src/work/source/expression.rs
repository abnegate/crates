//! Expressions and names in a masked scanner statement, as the walk rule
//! reads them.

use std::iter;

/// The methods that view a value as its bytes or its string.
const VIEWS: &[&str] = &[".as_bytes()", ".as_str()"];

/// A method chain or path in masked code, as `bytes[from..].iter()`,
/// `CharacterSet::Token` or `work::find(bytes, 0, found)?`: names joined by
/// `.` or `::`, each perhaps called or indexed, or a literal.
#[derive(Clone, Copy)]
pub(super) struct Expression<'a>(pub(super) &'a str);

impl<'a> Expression<'a> {
    /// The expression in `code` that ends at `end`, less the spaces before
    /// `end`.
    pub(super) fn before(code: &'a str, end: usize) -> Self {
        let bytes = code.as_bytes();
        let end = code[..end].trim_end().len();
        let mut start = end;
        loop {
            while let [.., b'?'] = bytes[..start] {
                start -= 1;
            }
            while let [.., b')' | b']'] = bytes[..start] {
                start = opening(bytes, start - 1);
            }
            if let [.., b'"' | b'\''] = bytes[..start] {
                start = literal_start(bytes, start - 1);
                break;
            }
            if let [.., b'>'] = bytes[..start] {
                start = opening(bytes, start - 1);
                if !code[..start].ends_with("::") {
                    break;
                }
                start -= 2;
            }
            if let [.., last, b'!'] = bytes[..start]
                && is_identifier(last)
            {
                start -= 1;
            }
            start = identifier_start(bytes, start);
            if code[..start].ends_with("::") {
                start -= 2;
            } else if code[..start].ends_with('.') && !code[..start].ends_with("..") {
                start -= 1;
            } else {
                break;
            }
        }
        Self(&code[start..end])
    }

    /// The expression in `code` that starts at `start`, past the spaces,
    /// references and dereferences before it.
    pub(super) fn after(code: &'a str, start: usize) -> Self {
        let bytes = code.as_bytes();
        let rest = code[start..].trim_start_matches([' ', '&', '*']);
        let start = code.len() - rest.strip_prefix("mut ").unwrap_or(rest).len();
        let mut end = start;
        while let Some(byte) = bytes.get(end) {
            end = match byte {
                b'(' | b'[' => closing(bytes, end) + 1,
                b'"' | b'\'' => literal_end(bytes, end),
                b'?' => end + 1,
                b':' if code[end..].starts_with("::") => end + 2,
                b'.' if !code[end..].starts_with("..") => end + 1,
                b'<' if code[..end].ends_with("::") => closing(bytes, end) + 1,
                _ if is_identifier(*byte) => identifier_end(bytes, end),
                _ => break,
            }
            .min(code.len());
        }
        Self(&code[start..end])
    }

    /// Its first name or path, with the calls and indexing on it, before any
    /// method: `bytes[from..]` for `bytes[from..].iter()`.
    pub(super) fn head(self) -> &'a str {
        let bytes = self.0.as_bytes();
        let mut index = 0;
        while let Some(byte) = bytes.get(index) {
            match byte {
                b'(' | b'[' | b'{' => index = closing(bytes, index),
                b'.' if bytes.get(index + 1) == Some(&b'.') => index += 1,
                b'.' => return &self.0[..index],
                _ => {}
            }
            index += 1;
        }
        self.0
    }

    /// Whether it starts from a table: a capitalised name reached without a
    /// call and indexed at most, as `QUOTES`, `INVISIBLE_CHARACTERS[range]`
    /// and `CharacterSet::Token` are. No `const` can hold the text, and the
    /// rule refuses a `static`, so the methods along it walk a constant.
    pub(super) fn is_on_table(self) -> bool {
        is_table(Self(self.stripped()).head())
    }

    /// Whether it is a constant: a literal, or a table perhaps viewed as its
    /// bytes or its string.
    pub(super) fn is_constant(self) -> bool {
        let expression = self.stripped();
        let head = Self(expression).head();
        let view = &expression[head.len()..];
        is_literal(expression) || (is_table(head) && (view.is_empty() || VIEWS.contains(&view)))
    }

    /// Whether it is what a `work` helper returns, which is an index, a flag
    /// or the indices it found and never the text: the value of any helper
    /// but `find_map`, whose value is what its closure returns.
    pub(super) fn is_work_result(self) -> bool {
        let expression = self.stripped().trim_end_matches('?');
        let expression = expression.strip_prefix("crate::").unwrap_or(expression);
        let Some(call) = expression.strip_prefix("work::") else {
            return false;
        };
        let name = identifier_end(call.as_bytes(), 0);
        &call[..name] != "find_map"
            && call[name..].starts_with('(')
            && closing(call.as_bytes(), name) + 1 == call.len()
    }

    /// Whether it is a number: a numeric literal, or a length.
    pub(super) fn is_number(self) -> bool {
        let expression = self.stripped();
        expression.ends_with(".len()")
            || expression.starts_with(|first: char| first.is_ascii_digit())
    }

    /// Whether it is recognisably the text or a part of it: one of `text`, the
    /// parameters that hold the text, a slice taken by a range, or the bytes or
    /// string of anything but a constant.
    pub(super) fn is_text(self, text: &[String]) -> bool {
        let expression = self.stripped().trim_end_matches('?');
        if text.iter().any(|name| name == expression) {
            return true;
        }
        if self.is_on_table() || is_literal(expression) {
            return false;
        }
        VIEWS.iter().any(|view| expression.ends_with(view)) || is_slice(expression)
    }

    /// It without the spaces around it, or the references, dereferences and
    /// `mut` before it.
    fn stripped(self) -> &'a str {
        let expression = self.0.trim().trim_start_matches(['&', '*']);
        expression.strip_prefix("mut ").unwrap_or(expression).trim()
    }
}

/// Every name in `code` that is neither a lifetime nor a label, with where it
/// starts.
pub(super) fn identifiers(code: &str) -> impl Iterator<Item = (usize, &str)> {
    let bytes = code.as_bytes();
    let mut index = 0;
    iter::from_fn(move || {
        while let Some(byte) = bytes.get(index) {
            let start = index;
            if !is_identifier(*byte) {
                index += 1;
                continue;
            }
            index = identifier_end(bytes, start);
            let word = byte.is_ascii_alphabetic() || *byte == b'_';
            let lifetime = start > 0 && bytes[start - 1] == b'\'';
            if word && !lifetime {
                return Some((start, &code[start..index]));
            }
        }
        None
    })
}

/// The arguments in `list`, split at its top-level commas, and at none inside
/// the parameters of a closure.
pub(super) fn arguments(list: &str) -> Vec<&str> {
    let bytes = list.as_bytes();
    let mut arguments = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while let Some(byte) = bytes.get(index) {
        match byte {
            b'(' | b'[' | b'{' => index = closing(bytes, index),
            b'|' if matches!(list[start..index].trim(), "" | "move") => {
                index += bytes[index + 1..]
                    .iter()
                    .position(|byte| *byte == b'|')
                    .map_or(bytes.len(), |end| end + 1);
            }
            b',' => {
                arguments.push(&list[start..index]);
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    arguments.push(list.get(start..).unwrap_or_default());
    arguments.retain(|argument| !argument.trim().is_empty());
    arguments
}

/// Whether `argument` is a closure.
pub(super) fn is_closure(argument: &str) -> bool {
    let argument = argument.trim();
    argument
        .strip_prefix("move ")
        .unwrap_or(argument)
        .starts_with('|')
}

/// Whether `byte` can be part of a name.
pub(super) fn is_identifier(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Where the name that ends at `end` starts, `end` when none does.
pub(super) fn identifier_start(bytes: &[u8], end: usize) -> usize {
    bytes[..end]
        .iter()
        .rposition(|byte| !is_identifier(*byte))
        .map_or(0, |separator| separator + 1)
}

/// Where the name that starts at `start` ends, `start` when none does.
pub(super) fn identifier_end(bytes: &[u8], start: usize) -> usize {
    bytes[start..]
        .iter()
        .position(|byte| !is_identifier(*byte))
        .map_or(bytes.len(), |end| start + end)
}

/// The index of the bracket that closes the one at `open`, or the length of
/// `bytes` when none does.
pub(super) fn closing(bytes: &[u8], open: usize) -> usize {
    let (opens, closes) = pair(bytes[open]);
    let mut depth = 0usize;
    for (index, byte) in bytes.iter().enumerate().skip(open) {
        if *byte == opens {
            depth += 1;
        } else if *byte == closes {
            depth -= 1;
            if depth == 0 {
                return index;
            }
        }
    }
    bytes.len()
}

/// The index of the bracket that opens the one at `close`, or 0 when none
/// does.
pub(super) fn opening(bytes: &[u8], close: usize) -> usize {
    let (opens, closes) = pair(bytes[close]);
    let mut depth = 0usize;
    for index in (0..=close).rev() {
        if bytes[index] == closes {
            depth += 1;
        } else if bytes[index] == opens {
            depth -= 1;
            if depth == 0 {
                return index;
            }
        }
    }
    0
}

/// The opening and closing bracket of the pair `bracket` belongs to.
fn pair(bracket: u8) -> (u8, u8) {
    match bracket {
        b'(' | b')' => (b'(', b')'),
        b'[' | b']' => (b'[', b']'),
        b'{' | b'}' => (b'{', b'}'),
        _ => (b'<', b'>'),
    }
}

/// Whether `head` is a table: a capitalised name or path, indexed at most.
fn is_table(head: &str) -> bool {
    let path = head
        .find(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '_' | ':'))
        })
        .unwrap_or(head.len());
    let name = head[..path].rsplit("::").next().unwrap_or_default();
    let bytes = &head.as_bytes()[path..];
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'[' {
            return false;
        }
        index = closing(bytes, index) + 1;
    }
    name.starts_with(|first: char| first.is_ascii_uppercase())
}

/// Whether `expression` is a literal: a number, a flag, or a string, byte or
/// character literal.
fn is_literal(expression: &str) -> bool {
    expression.starts_with(|first: char| first.is_ascii_digit())
        || matches!(expression, "true" | "false")
        || expression
            .trim_start_matches(['b', 'c', 'r', '#'])
            .starts_with(['"', '\''])
}

/// Whether `expression` ends in a slice taken by a range, as `bytes[from..]`
/// or `text.get(start..end)` do.
fn is_slice(expression: &str) -> bool {
    let bytes = expression.as_bytes();
    let Some(close) = bytes.len().checked_sub(1) else {
        return false;
    };
    let indexed = bytes[close] == b']';
    if !indexed && bytes[close] != b')' {
        return false;
    }
    let open = opening(bytes, close);
    expression[open..close].contains("..") && (indexed || expression[..open].ends_with(".get"))
}

/// Where the literal that ends with the quote at `close` starts, with its
/// prefix.
fn literal_start(bytes: &[u8], close: usize) -> usize {
    let mut start = bytes[..close]
        .iter()
        .rposition(|byte| *byte == bytes[close])
        .unwrap_or(close);
    while let [.., b'#'] = bytes[..start] {
        start -= 1;
    }
    if let [.., b'r'] = bytes[..start] {
        start -= 1;
    }
    if let [.., b'b' | b'c'] = bytes[..start] {
        start -= 1;
    }
    start
}

/// Where the literal whose opening quote is at `open` ends, past its closing
/// quote.
fn literal_end(bytes: &[u8], open: usize) -> usize {
    bytes[open + 1..]
        .iter()
        .position(|byte| *byte == bytes[open])
        .map_or(bytes.len(), |close| open + close + 2)
}
