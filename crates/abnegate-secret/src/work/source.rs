//! The rule that makes the count complete: outside their tests, the scanners
//! walk no sequence but through `work`.
//!
//! A statement walks when it loops, or calls a method that reads a sequence
//! element by element on anything but a constant or `work`: a table named in
//! SCREAMING_CASE or reached through a type is not the text, so a walk over it
//! needs no count, and what a `work` helper yields it has counted. Each
//! statement that walks something other than the text anyway is on
//! [`ALLOWED`], with what it walks instead.

use std::fs;
use std::path::Path;

/// Every scanner file, by its path under `src`.
const SCANNERS: &[(&str, &str)] = &[
    ("redact.rs", include_str!("../redact.rs")),
    (
        "redact/character_set.rs",
        include_str!("../redact/character_set.rs"),
    ),
    (
        "redact/credential.rs",
        include_str!("../redact/credential.rs"),
    ),
    ("sanitize.rs", include_str!("../sanitize.rs")),
    (
        "sanitize/terminators.rs",
        include_str!("../sanitize/terminators.rs"),
    ),
];

/// The directories whose every file is a scanner file.
const SCANNER_DIRECTORIES: &[&str] = &["redact", "sanitize"];

const LOOPS: &[&str] = &["for", "loop", "while"];

/// Methods that read a sequence element by element, from `std`'s slices,
/// strings and iterators.
const WALKS: &[&str] = &[
    "all",
    "any",
    "bytes",
    "char_indices",
    "chars",
    "chunks",
    "collect",
    "contains",
    "count",
    "filter_map",
    "find",
    "find_map",
    "flat_map",
    "fold",
    "for_each",
    "into_iter",
    "iter",
    "last",
    "lines",
    "map_while",
    "match_indices",
    "matches",
    "next",
    "nth",
    "position",
    "product",
    "rchunks",
    "reduce",
    "rev",
    "rfind",
    "rfold",
    "rmatch_indices",
    "rmatches",
    "rposition",
    "rsplit",
    "rsplit_once",
    "rsplit_terminator",
    "rsplitn",
    "scan",
    "skip",
    "skip_while",
    "split",
    "split_ascii_whitespace",
    "split_inclusive",
    "split_once",
    "split_terminator",
    "split_whitespace",
    "splitn",
    "step_by",
    "sum",
    "take_while",
    "trim",
    "trim_ascii",
    "trim_ascii_end",
    "trim_ascii_start",
    "trim_end",
    "trim_end_matches",
    "trim_matches",
    "trim_start",
    "trim_start_matches",
    "try_fold",
    "try_for_each",
    "windows",
];

/// Byte searches from outside `std`.
const SEARCHES: &[&str] = &["memchr", "memmem"];

/// A statement that walks something other than the text.
struct Allowance {
    file: &'static str,
    code: &'static str,
    walks: &'static str,
}

const ALLOWED: &[Allowance] = &[
    Allowance {
        file: "redact.rs",
        code: "while index < bytes.len() {",
        walks: "the cursor every counted walk starts from",
    },
    Allowance {
        file: "redact.rs",
        code: "for (start, end) in spans {",
        walks: "the spans redacted, which are disjoint",
    },
    Allowance {
        file: "redact.rs",
        code: "if !label.contains(PEM_PRIVATE) {",
        walks: "the label a counted walk has just delimited",
    },
    Allowance {
        file: "redact.rs",
        code: "words.iter().any(|candidate| word.eq_ignore_ascii_case(candidate.as_bytes()))",
        walks: "a table of key words",
    },
    Allowance {
        file: "redact.rs",
        code: "for _ in 0..2 {",
        walks: "the two segments after a token's header",
    },
    Allowance {
        file: "redact.rs",
        code: "&& words.iter().any(|word| contains(key, word.as_bytes()))",
        walks: "a table of key words",
    },
    Allowance {
        file: "redact.rs",
        code: "while list < lists.len() {",
        walks: "tables of key words, at compile time",
    },
    Allowance {
        file: "redact.rs",
        code: "while word < lists[list].len() {",
        walks: "a table of key words, at compile time",
    },
    Allowance {
        file: "redact.rs",
        code: "haystack.windows(needle.len()).any(|window| window == needle)",
        walks: "a key word and the lowercased key, a copy of at most KEY_LOOKBEHIND bytes",
    },
    Allowance {
        file: "redact.rs",
        code: "if counts.iter().filter(|count| **count > 0).count() < MINIMUM_DISTINCT_CHARACTERS {",
        walks: "the 128 counts of a candidate's bytes",
    },
    Allowance {
        file: "redact.rs",
        code: "let entropy: f64 = counts.iter().filter(|count| **count > 0).map(|count| { \
               let probability = f64::from(*count) / length; \
               -probability * probability.log2() }).sum();",
        walks: "the 128 counts of a candidate's bytes",
    },
    Allowance {
        file: "redact/character_set.rs",
        code: "work::run(bytes, from, |byte| self.contains(byte))",
        walks: "this character set, for one byte",
    },
    Allowance {
        file: "redact/credential.rs",
        code: "while index < credentials.len() {",
        walks: "the credential table, at compile time",
    },
    Allowance {
        file: "sanitize.rs",
        code: "while index < bytes.len() {",
        walks: "the cursor every counted walk starts from",
    },
    Allowance {
        file: "sanitize.rs",
        code: "let Some(character) = text[index..].chars().next() else {",
        walks: "the one character at the cursor",
    },
    Allowance {
        file: "sanitize.rs",
        code: "while byte < 0x80 {",
        walks: "the ASCII bytes, at compile time",
    },
    Allowance {
        file: "sanitize.rs",
        code: "while range < INVISIBLE_CHARACTERS.len() {",
        walks: "the table of invisible characters, at compile time",
    },
    Allowance {
        file: "sanitize.rs",
        code: "while lead <= lead_byte(*range.end()) {",
        walks: "the lead bytes of a range of characters, at compile time",
    },
];

/// Assert that the scanners walk the text only through `work`, so that its
/// count sees every walk.
#[track_caller]
pub(super) fn assert_every_walk_counted() {
    let unreported = unreported();
    assert!(
        unreported.is_empty(),
        "a walk outside work is invisible to its count:\n{}",
        unreported.join("\n")
    );
}

/// Every statement of the scanners that walks outside `work` and is not
/// allowed, and every allowance that does not match exactly one statement.
fn unreported() -> Vec<String> {
    let mut matched = vec![0; ALLOWED.len()];
    let mut unreported = Vec::new();
    for (file, source) in SCANNERS {
        for (line, code) in statements(source) {
            if !walks(&code) {
                continue;
            }
            match ALLOWED
                .iter()
                .position(|allowance| allowance.file == *file && allowance.code == code)
            {
                Some(allowance) => matched[allowance] += 1,
                None => unreported.push(format!("src/{file}:{line} walks outside work: {code}")),
            }
        }
    }
    for (allowance, matched) in ALLOWED.iter().zip(matched) {
        if matched != 1 {
            unreported.push(format!(
                "src/{}: `{}` is allowed once, as it walks {}, but occurs {matched} times",
                allowance.file, allowance.code, allowance.walks
            ));
        }
    }
    unreported
}

/// The statements of `source` outside its test modules, each with the line
/// it starts on: its lines joined while a bracket is open or a method chain
/// continues, with comments and the contents of literals blanked.
fn statements(source: &str) -> Vec<(usize, String)> {
    let masked = masked(source);
    let lines: Vec<&str> = masked.lines().collect();
    let mut statements: Vec<(usize, String)> = Vec::new();
    let mut depth = 0;
    let mut index = 0;
    while let Some(line) = lines.get(index) {
        let line = line.trim();
        if line == "#[cfg(test)]"
            && lines
                .get(index + 1)
                .is_some_and(|next| next.starts_with("mod "))
        {
            index += lines[index..]
                .iter()
                .position(|line| *line == "}")
                .map_or(lines.len(), |end| end + 1);
            continue;
        }
        if !line.is_empty() {
            match statements.last_mut() {
                Some((_, statement)) if depth > 0 || line.starts_with('.') => {
                    if !line.starts_with('.') {
                        statement.push(' ');
                    }
                    statement.push_str(line);
                }
                _ => statements.push((index + 1, line.to_owned())),
            }
            depth += line.matches(['(', '[']).count() as isize;
            depth -= line.matches([')', ']']).count() as isize;
        }
        index += 1;
    }
    statements
}

/// `source` with every comment, and the contents of every string and
/// character literal, replaced by spaces, keeping its lines where they are.
fn masked(source: &str) -> String {
    let characters: Vec<char> = source.chars().collect();
    let mut masked = characters.clone();
    let mut index = 0;
    while let Some(character) = characters.get(index) {
        let blank = match character {
            '/' if characters.get(index + 1) == Some(&'/') => {
                index..closing(&characters, index, '\n')
            }
            '"' => index + 1..closing(&characters, index + 1, '"'),
            '\'' if characters.get(index + 1) == Some(&'\\')
                || characters.get(index + 2) == Some(&'\'') =>
            {
                index + 1..closing(&characters, index + 1, '\'')
            }
            _ => {
                index += 1;
                continue;
            }
        };
        index = blank.end + 1;
        for masked in &mut masked[blank] {
            if *masked != '\n' {
                *masked = ' ';
            }
        }
    }
    masked.into_iter().collect()
}

/// The index of the `end` that closes what opens before `from`, skipping
/// escaped characters.
fn closing(characters: &[char], from: usize, end: char) -> usize {
    let mut index = from;
    while let Some(character) = characters.get(index) {
        match character {
            _ if *character == end => return index,
            '\\' if end != '\n' => index += 2,
            _ => index += 1,
        }
    }
    characters.len()
}

/// Whether `code` loops, or calls a walk on anything but a constant.
fn walks(code: &str) -> bool {
    code.split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .any(|word| LOOPS.contains(&word))
        || SEARCHES.iter().any(|search| code.contains(search))
        || WALKS.iter().any(|method| calls(code, method))
}

/// Whether `code` calls `method`, as `receiver.method(` on a receiver that
/// neither is a constant nor comes from `work`, or as `module::method(` from a
/// module other than `work`.
fn calls(code: &str, method: &str) -> bool {
    code.match_indices(method).any(|(at, _)| {
        let arguments = &code[at + method.len()..];
        let called = arguments.starts_with('(') || arguments.starts_with("::<");
        called
            && match code.as_bytes()[..at] {
                [.., b'.'] => {
                    let root = root(code, at - 1);
                    root != "work" && !root.starts_with(|first: char| first.is_ascii_uppercase())
                }
                [.., b':', b':'] => name_before(code, at - 2) != "work",
                _ => false,
            }
    })
}

/// The name a method chain or path that ends at `end` starts from, past the
/// calls and indexing along it: `bytes` for `bytes[from..].iter()`, and an
/// empty name for a chain that starts from a bracket, as `(from..to).map(f)`.
fn root(code: &str, end: usize) -> &str {
    let bytes = code.as_bytes();
    let mut end = end;
    loop {
        while end > 0 && matches!(bytes[end - 1], b')' | b']') {
            end = opening(bytes, end - 1);
        }
        let name = name_before(code, end);
        let start = end - name.len();
        match bytes[..start] {
            [.., b'.'] => end = start - 1,
            [.., b':', b':'] => end = start - 2,
            _ => return name,
        }
    }
}

/// The identifier that ends at `end`, empty when none does.
fn name_before(code: &str, end: usize) -> &str {
    let start = code.as_bytes()[..end]
        .iter()
        .rposition(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
        .map_or(0, |separator| separator + 1);
    &code[start..end]
}

/// The index of the bracket that `bytes[close]` closes.
fn opening(bytes: &[u8], close: usize) -> usize {
    let mut depth = 0;
    for index in (0..=close).rev() {
        match bytes[index] {
            b')' | b']' => depth += 1,
            b'(' | b'[' => {
                depth -= 1;
                if depth == 0 {
                    return index;
                }
            }
            _ => {}
        }
    }
    0
}

#[test]
fn every_walk_in_the_scanners_goes_through_work() {
    assert_every_walk_counted();
}

#[test]
fn every_scanner_file_is_checked() {
    for directory in SCANNER_DIRECTORIES {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join(directory);
        for entry in fs::read_dir(path).unwrap() {
            let name = format!(
                "{directory}/{}",
                entry.unwrap().file_name().to_string_lossy()
            );
            assert!(
                SCANNERS.iter().any(|(file, _)| *file == name),
                "src/{name} is a scanner file the walk rule does not check"
            );
        }
    }
}

#[test]
fn refuses_every_way_of_walking_the_text_outside_work() {
    for code in [
        "(from..bytes.len()).find_map(|index| string_terminator_end(bytes, index))",
        "while index < bytes.len() && bytes[index] != BELL {",
        "for index in from..bytes.len() {",
        "loop {",
        "bytes[from..].iter().position(|byte| *byte == BELL)",
        "if !bytes[from..].contains(&BELL) {",
        "let end = rest.find('\\n')?;",
        "text.lines().next()",
        "str::find(text, closing)",
        "<[u8]>::iter(bytes).any(|byte| *byte == BELL)",
        "CREDENTIALS.iter().any(|_| bytes[index..].iter().any(|byte| *byte == BELL))",
        "memchr::memchr(BELL, &bytes[from..])",
        "(from..bytes.len()).map(|index| usize::from(bytes[index] == BELL)).sum::<usize>()",
    ] {
        assert!(walks(&masked(code)), "{code} was not refused");
    }
}

#[test]
fn allows_walks_through_work_and_over_constants() {
    for code in [
        "work::find_map(bytes, from, |index| string_terminator_end(bytes, index))",
        "crate::work::find(bytes, from, |index| bytes[index] == BELL)",
        "work::occurrences(text, closing).find(|at| closes(text, at))",
        "let end = work::run(bytes, from, |byte| CharacterSet::Token.contains(byte));",
        "QUOTES.contains(&byte)",
        "INVISIBLE_CHARACTERS[range].contains(&character)",
        "CREDENTIALS.iter().find_map(|credential| credential.end_at(bytes, index))",
        "bytes[index..].starts_with(URL_SCHEME_SEPARATOR)",
        "const FOR: &str = \"for (x) in .iter() while\";",
        "let found = 'x'; // for each byte, while it lasts",
    ] {
        assert!(!walks(&masked(code)), "{code} was refused");
    }
}
