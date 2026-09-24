//! The statements of a scanner file, as the walk rule reads them.

use std::ops::Range;

use crate::work::source::expression::arguments;
use crate::work::source::expression::closing;
use crate::work::source::expression::identifier_end;
use crate::work::source::expression::identifiers;
use crate::work::source::expression::is_identifier;

/// Operators rustfmt breaks a line before, which continue the statement the
/// line before started.
const CONTINUATIONS: &[&str] = &[
    "!= ", "% ", "& ", "&& ", "* ", "+ ", "- ", "/ ", "< ", "<= ", "== ", "> ", ">= ", "?", "^ ",
    "as ", "| ", "|| ",
];

/// A statement of a scanner file outside its test modules: its lines joined
/// while a bracket is open, a method chain continues or a line starts with a
/// binary operator, with its comments and the contents of its literals
/// blanked.
pub(super) struct Statement {
    /// The line it starts on.
    pub(super) line: usize,
    pub(super) code: String,
    /// The innermost function it is in, empty outside any.
    pub(super) function: String,
    /// The parameters of that function whose type can hold the text.
    pub(super) text: Vec<String>,
}

/// The statements of `source`.
pub(super) fn statements(source: &str) -> Vec<Statement> {
    let code = without_test_modules(&masked(source));
    let mut joined: Vec<(usize, String)> = Vec::new();
    let mut depth = 0isize;
    for (index, line) in code.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let continues = depth > 0
            || line.starts_with('.')
            || CONTINUATIONS
                .iter()
                .any(|operator| line.starts_with(operator));
        match joined.last_mut() {
            Some((_, statement)) if continues => {
                if !line.starts_with('.') {
                    statement.push(' ');
                }
                statement.push_str(line);
            }
            _ => joined.push((index + 1, line.to_owned())),
        }
        depth += line.matches(['(', '[']).count() as isize;
        depth -= line.matches([')', ']']).count() as isize;
    }
    in_functions(joined)
}

/// `source` with its comments, and the contents of its string and character
/// literals, blanked, keeping its lines where they are. Block comments nest,
/// and a raw string ends at the first quote followed by its hashes.
pub(super) fn masked(source: &str) -> String {
    let characters: Vec<char> = source.chars().collect();
    let mut masked = characters.clone();
    let mut index = 0;
    while index < characters.len() {
        match hidden(&characters, index) {
            Some((blanked, next)) => {
                blank(&mut masked[blanked]);
                index = next;
            }
            None => index += 1,
        }
    }
    masked.into_iter().collect()
}

/// The part to blank of the comment or literal that starts at `index`, if one
/// does, and where reading resumes after it.
fn hidden(characters: &[char], index: usize) -> Option<(Range<usize>, usize)> {
    let at = |offset: usize| characters.get(index + offset).copied();
    match characters[index] {
        '/' if at(1) == Some('/') => {
            let end = characters[index..]
                .iter()
                .position(|character| *character == '\n')
                .map_or(characters.len(), |end| index + end);
            Some((index..end, end))
        }
        '/' if at(1) == Some('*') => {
            let end = block_comment_end(characters, index);
            Some((index..end, end))
        }
        '"' => {
            let end = quote_end(characters, index + 1, '"');
            Some((index + 1..end, end + 1))
        }
        '\'' if at(1) == Some('\\') || at(2) == Some('\'') => {
            let end = quote_end(characters, index + 1, '\'');
            Some((index + 1..end, end + 1))
        }
        'b' | 'c' | 'r' if index == 0 || !is_word(characters[index - 1]) => {
            raw_string(characters, index)
        }
        _ => None,
    }
}

/// The contents of the raw string that starts at `index`, if one does, and
/// where reading resumes after it. Nothing in a raw string escapes.
fn raw_string(characters: &[char], index: usize) -> Option<(Range<usize>, usize)> {
    let mut quote = index;
    if matches!(characters[quote], 'b' | 'c') {
        quote += 1;
    }
    if characters.get(quote) != Some(&'r') {
        return None;
    }
    quote += 1;
    let hashes = characters[quote..]
        .iter()
        .take_while(|character| **character == '#')
        .count();
    quote += hashes;
    if characters.get(quote) != Some(&'"') {
        return None;
    }
    let contents = quote + 1;
    let end = (contents..characters.len())
        .find(|end| {
            characters[*end] == '"'
                && characters[end + 1..]
                    .iter()
                    .take(hashes)
                    .filter(|character| **character == '#')
                    .count()
                    == hashes
        })
        .unwrap_or(characters.len());
    Some((contents..end, end + 1 + hashes))
}

/// Where the block comment that starts at `start` ends, past the `*/` that
/// closes it and every comment nested in it.
fn block_comment_end(characters: &[char], start: usize) -> usize {
    let mut depth = 0usize;
    let mut index = start;
    while index + 1 < characters.len() {
        match (characters[index], characters[index + 1]) {
            ('/', '*') => {
                depth += 1;
                index += 2;
            }
            ('*', '/') => {
                depth -= 1;
                index += 2;
                if depth == 0 {
                    return index;
                }
            }
            _ => index += 1,
        }
    }
    characters.len()
}

/// The index of the `quote` that closes a literal whose contents start at
/// `from`, past escaped characters.
fn quote_end(characters: &[char], from: usize, quote: char) -> usize {
    let mut index = from;
    while let Some(character) = characters.get(index) {
        match character {
            '\\' => index += 2,
            _ if *character == quote => return index,
            _ => index += 1,
        }
    }
    characters.len()
}

/// `code`, masked, with every module compiled only for tests blanked: `mod
/// name { .. }` to the brace that closes it, and `mod name;` to its
/// semicolon.
fn without_test_modules(code: &str) -> String {
    let mut characters: Vec<char> = code.chars().collect();
    let mut index = 0;
    while index < characters.len() {
        match test_module_end(&characters, index) {
            Some(end) => {
                blank(&mut characters[index..end]);
                index = end;
            }
            None => index += 1,
        }
    }
    characters.into_iter().collect()
}

/// Where the test module whose `#[cfg(test)]` starts at `index` ends, if it
/// starts one.
fn test_module_end(characters: &[char], index: usize) -> Option<usize> {
    if characters[index] != '#' {
        return None;
    }
    let mut cursor = tokens(characters, index, &["#", "[", "cfg", "(", "test", ")", "]"])?;
    while let Some(open) = tokens(characters, cursor, &["#", "["]) {
        cursor = bracket_end(characters, open - 1);
    }
    if let Some(after) = tokens(characters, cursor, &["pub"]) {
        cursor = tokens(characters, after, &["("])
            .map_or(after, |open| bracket_end(characters, open - 1));
    }
    cursor = skip_whitespace(characters, tokens(characters, cursor, &["mod"])?);
    let name = characters[cursor..]
        .iter()
        .take_while(|character| is_word(**character))
        .count();
    if name == 0 {
        return None;
    }
    cursor = skip_whitespace(characters, cursor + name);
    match characters.get(cursor)? {
        ';' => Some(cursor + 1),
        '{' => Some(bracket_end(characters, cursor)),
        _ => None,
    }
}

/// Where `expected` ends if it starts at `index`, a token at a time with any
/// whitespace between them.
fn tokens(characters: &[char], index: usize, expected: &[&str]) -> Option<usize> {
    let mut cursor = index;
    for token in expected {
        cursor = skip_whitespace(characters, cursor);
        for character in token.chars() {
            if characters.get(cursor) != Some(&character) {
                return None;
            }
            cursor += 1;
        }
        let word = token.chars().all(is_word);
        if word && characters.get(cursor).is_some_and(|next| is_word(*next)) {
            return None;
        }
    }
    Some(cursor)
}

/// Where the brackets that open at `open` close, past the closing bracket.
fn bracket_end(characters: &[char], open: usize) -> usize {
    let closes = match characters[open] {
        '(' => ')',
        '[' => ']',
        _ => '}',
    };
    let mut depth = 0usize;
    for (index, character) in characters.iter().enumerate().skip(open) {
        if *character == characters[open] {
            depth += 1;
        } else if *character == closes {
            depth -= 1;
            if depth == 0 {
                return index + 1;
            }
        }
    }
    characters.len()
}

fn skip_whitespace(characters: &[char], index: usize) -> usize {
    index
        + characters[index.min(characters.len())..]
            .iter()
            .take_while(|character| character.is_whitespace())
            .count()
}

fn is_word(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Replace `characters` by spaces, keeping their line breaks.
fn blank(characters: &mut [char]) {
    for character in characters
        .iter_mut()
        .filter(|character| **character != '\n')
    {
        *character = ' ';
    }
}

/// `joined`, each statement with the function it is in.
fn in_functions(joined: Vec<(usize, String)>) -> Vec<Statement> {
    let mut open: Vec<(isize, String, Vec<String>)> = Vec::new();
    let mut declared: Option<(String, Vec<String>)> = None;
    let mut depth = 0isize;
    joined
        .into_iter()
        .map(|(line, code)| {
            let mut innermost = open
                .last()
                .map(|(_, name, text)| (name.clone(), text.clone()));
            let mut nesting = 0isize;
            for (index, byte) in code.bytes().enumerate() {
                match byte {
                    b'(' | b'[' => nesting += 1,
                    b')' | b']' => nesting -= 1,
                    b'{' => {
                        depth += 1;
                        if let Some((name, text)) = declared.take() {
                            innermost = Some((name.clone(), text.clone()));
                            open.push((depth, name, text));
                        }
                    }
                    b'}' => {
                        if open.last().is_some_and(|(body, ..)| *body == depth) {
                            open.pop();
                        }
                        depth -= 1;
                    }
                    b';' if nesting == 0 => declared = None,
                    b'f' if is_keyword(&code, index, "fn") => {
                        if let Some(function) = declaration(&code[index + 2..]) {
                            declared = Some(function);
                        }
                    }
                    _ => {}
                }
            }
            let (function, text) = innermost.unwrap_or_default();
            Statement {
                line,
                code,
                function,
                text,
            }
        })
        .collect()
}

/// Whether `keyword` is the word at `index` in `code`.
fn is_keyword(code: &str, index: usize, keyword: &str) -> bool {
    let bytes = code.as_bytes();
    code[index..].starts_with(keyword)
        && (index == 0 || !(is_identifier(bytes[index - 1]) || bytes[index - 1] == b'\''))
        && bytes
            .get(index + keyword.len())
            .is_none_or(|next| !is_identifier(*next))
}

/// The name and the text parameters of the function whose declaration follows
/// `fn` in `rest`, if one does.
fn declaration(rest: &str) -> Option<(String, Vec<String>)> {
    let rest = rest.trim_start();
    let bytes = rest.as_bytes();
    let name = identifier_end(bytes, 0);
    if name == 0 {
        return None;
    }
    let mut open = name;
    if bytes.get(open) == Some(&b'<') {
        open = closing(bytes, open) + 1;
    }
    if bytes.get(open) != Some(&b'(') {
        return None;
    }
    let parameters = rest.get(open + 1..closing(bytes, open)).unwrap_or_default();
    let text = arguments(parameters)
        .into_iter()
        .filter_map(|parameter| {
            let (pattern, kind) = parameter.split_once(':')?;
            let pattern = pattern.trim();
            let pattern = pattern.strip_prefix("mut ").unwrap_or(pattern);
            let simple = identifier_end(pattern.as_bytes(), 0) == pattern.len();
            (simple && holds_text(kind)).then(|| pattern.to_owned())
        })
        .collect();
    Some((rest[..name].to_owned(), text))
}

/// Whether a value of type `kind` can hold the text: it is, or is built of, a
/// string or bytes.
fn holds_text(kind: &str) -> bool {
    let compact = kind.replace(' ', "");
    identifiers(kind).any(|(_, word)| matches!(word, "str" | "String" | "Cow"))
        || compact.contains("[u8]")
        || compact.contains("Vec<u8>")
}

const WALK: &str = "bytes.iter().position(|byte| *byte == 0).unwrap_or(0)";

fn keeps(source: &str, code: &str) -> bool {
    statements(source)
        .iter()
        .any(|statement| statement.code == code)
}

#[test]
fn keeps_the_code_after_what_a_scanner_could_hide_it_behind() {
    for (hiding, source) in [
        (
            "an out-of-line test module",
            "mod terminators;\n#[cfg(test)]\nmod tests;\n\nfn scan(bytes: &[u8]) -> usize {\n    bytes.iter().position(|byte| *byte == 0).unwrap_or(0)\n}\n",
        ),
        (
            "a test module closed by a commented brace",
            "#[cfg(test)]\nmod tests {\n    fn t() {}\n} // tests\n\nfn scan(bytes: &[u8]) -> usize {\n    bytes.iter().position(|byte| *byte == 0).unwrap_or(0)\n}\n\nfn other() {\n}\n",
        ),
        (
            "a raw string that ends in a backslash",
            "const SEPARATOR: &str = r\"\\\";\nfn scan(bytes: &[u8]) -> usize {\n    bytes.iter().position(|byte| *byte == 0).unwrap_or(0)\n}\nconst QUOTE: &str = \"x\";\n",
        ),
        (
            "a comparison broken before its operator",
            "fn scan(bytes: &[u8]) -> usize {\n    let same = bytes[0..1]\n        == bytes[1..2];\n    bytes.iter().position(|byte| *byte == 0).unwrap_or(0)\n}\n",
        ),
        (
            "a block comment that holds a quote",
            "/* \" */\nfn scan(bytes: &[u8]) -> usize {\n    bytes.iter().position(|byte| *byte == 0).unwrap_or(0)\n}\nconst QUOTE: &str = \"x\";\n",
        ),
    ] {
        assert!(keeps(source, WALK), "the walk after {hiding} was skipped");
    }
}

#[test]
fn joins_a_line_that_starts_with_a_binary_operator_to_the_one_before() {
    let source = "let same = bytes[0..1]\n    == bytes[1..2];\n*userinfo = None;\n";
    let codes: Vec<String> = statements(source)
        .into_iter()
        .map(|statement| statement.code)
        .collect();
    assert_eq!(
        codes,
        [
            "let same = bytes[0..1] == bytes[1..2];",
            "*userinfo = None;"
        ]
    );
}

#[test]
fn keeps_every_statement_of_a_scanner_with_an_out_of_line_test_module() {
    let source = include_str!("../../sanitize.rs").replacen(
        "mod terminators;\n",
        "mod terminators;\n#[cfg(test)]\nmod tests;\n",
        1,
    );
    assert!(keeps(
        &source,
        "let stripped = strip_control_sequences(text);"
    ));
}

#[test]
fn skips_test_modules_and_nothing_after_them() {
    let source = concat!(
        "#[cfg(test)]\n#[allow(unused)]\npub(crate) mod helpers {\n    fn walk(bytes: &[u8]) {\n",
        "        let _ = '}';\n        loop {}\n    }\n} fn after() {}\n",
        "#[cfg(any(test, feature = \"x\"))]\nmod also_built {\n    fn kept() {}\n}\n",
    );
    let codes: Vec<String> = statements(source)
        .into_iter()
        .map(|statement| statement.code)
        .collect();
    assert_eq!(
        codes,
        [
            "fn after() {}",
            "#[cfg(any(test, feature = \" \"))]",
            "mod also_built {",
            "fn kept() {}",
            "}"
        ]
    );
}

#[test]
fn masks_comments_and_the_contents_of_literals() {
    for (source, expected) in [
        ("a /* b /* c */ d */ e", "a                   e"),
        ("a // b \" c\nd", "a         \nd"),
        ("r#\"a \"b\" c\"# d", "r#\"       \"# d"),
        ("br\"\\\" d", "br\" \" d"),
        ("b'\\'' '\"' d", "b'  ' ' ' d"),
        ("\"a\\\"b\" d", "\"    \" d"),
        ("fn f<'a>(x: &'a str) {}", "fn f<'a>(x: &'a str) {}"),
        ("for\"x\"", "for\" \""),
    ] {
        assert_eq!(masked(source), expected, "{source}");
    }
}

#[test]
fn knows_the_function_each_statement_is_in() {
    let source = concat!(
        "const LIMIT: usize = 1;\n",
        "fn outer(text: &str, bytes: &[u8], index: usize) -> bool {\n",
        "    let inner = |byte: u8| { byte == 0 };\n",
        "    fn nested(list: fn(u8) -> bool) -> usize {\n        1\n    }\n",
        "    inner(bytes[index])\n",
        "}\n",
        "fn after() {}\n",
    );
    let functions: Vec<(String, String, Vec<String>)> = statements(source)
        .into_iter()
        .map(|statement| (statement.code, statement.function, statement.text))
        .collect();
    let text = vec![String::from("text"), String::from("bytes")];
    assert_eq!(
        functions,
        [
            ("const LIMIT: usize = 1;".into(), String::new(), Vec::new()),
            (
                "fn outer(text: &str, bytes: &[u8], index: usize) -> bool {".into(),
                "outer".into(),
                text.clone()
            ),
            (
                "let inner = |byte: u8| { byte == 0 };".into(),
                "outer".into(),
                text.clone()
            ),
            (
                "fn nested(list: fn(u8) -> bool) -> usize {".into(),
                "nested".into(),
                Vec::new()
            ),
            ("1".into(), "nested".into(), Vec::new()),
            ("}".into(), "nested".into(), Vec::new()),
            ("inner(bytes[index])".into(), "outer".into(), text.clone()),
            ("}".into(), "outer".into(), text),
            ("fn after() {}".into(), "after".into(), Vec::new()),
        ]
    );
}
