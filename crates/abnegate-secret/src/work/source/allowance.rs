//! The statements the walk rule refuses that walk something other than the
//! text.

use crate::work::source::statement::masked;

/// A statement the walk rule refuses that walks something other than the
/// text.
pub(super) struct Allowance {
    /// The scanner file it is in, by its path under `src`.
    pub(super) file: &'static str,
    /// The statement, compared without its whitespace, the contents of its
    /// literals, or a comma before a closing bracket, so that a reformat
    /// leaves it matching.
    pub(super) code: &'static str,
    /// What it walks instead of the text.
    pub(super) walks: &'static str,
}

impl Allowance {
    /// Whether this allows `code`, a masked statement of `file`.
    pub(super) fn allows(&self, file: &str, code: &str) -> bool {
        self.file == file && normalised(&masked(self.code)) == normalised(code)
    }
}

/// `code` without its whitespace, or a comma before a closing bracket.
fn normalised(code: &str) -> String {
    let compact: String = code
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    compact
        .replace(",)", ")")
        .replace(",]", "]")
        .replace(",}", "}")
}

pub(super) const ALLOWED: &[Allowance] = &[
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
        code: "output.push_str(&text[cursor..start]);",
        walks: "the text between two redacted spans, copied once",
    },
    Allowance {
        file: "redact.rs",
        code: "output.push_str(&text[cursor..]);",
        walks: "the text after the last redacted span, copied once",
    },
    Allowance {
        file: "redact.rs",
        code: "if !label.contains(PEM_PRIVATE) {",
        walks: "the label a counted walk has just delimited",
    },
    Allowance {
        file: "redact.rs",
        code: "let closing = format!(\"{PEM_END}{label}\");",
        walks: "the label a counted walk has just delimited",
    },
    Allowance {
        file: "redact.rs",
        code: "let introducer = &bytes[word.clone()];",
        walks: "nothing: `word` is a range of indices",
    },
    Allowance {
        file: "redact.rs",
        code: "words.iter().any(|candidate| word.eq_ignore_ascii_case(candidate.as_bytes()))",
        walks: "a table of key words",
    },
    Allowance {
        file: "redact.rs",
        code: "let prefix = AWS_ACCESS_KEY_PREFIXES.iter().find(|prefix| bytes[index..].starts_with(prefix))?;",
        walks: "a table of prefixes, each compared for its own length",
    },
    Allowance {
        file: "redact.rs",
        code: "for _ in 0..2 {",
        walks: "the two segments after a token's header",
    },
    Allowance {
        file: "redact.rs",
        code: "work::each(&bytes[start..end], |byte| { if byte.is_ascii_alphanumeric() { \
               buffer[length] = byte.to_ascii_lowercase(); length += 1; } });",
        walks: "one byte of the key, lowercased as it is copied",
    },
    Allowance {
        file: "redact.rs",
        code: "!REFERENCE_KEY_SUFFIXES.iter().any(|suffix| key.ends_with(suffix.as_bytes())) \
               && words.iter().any(|word| contains(key, word.as_bytes()))",
        walks: "tables of suffixes and key words, and the lowercased key, a copy of at most \
                KEY_LOOKBEHIND bytes",
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
        file: "redact.rs",
        code: "work::each(candidate, |byte| { current = if byte.is_ascii_alphanumeric() { \
               current + 1 } else { 0 }; longest = longest.max(current); });",
        walks: "the length of a run, a number",
    },
    Allowance {
        file: "redact/character_set.rs",
        code: "work::run(bytes, from, |byte| self.contains(byte))",
        walks: "this character set, for one byte",
    },
    Allowance {
        file: "redact/credential.rs",
        code: "LEADING_BYTES.get(usize::from(byte.to_ascii_lowercase())).is_some_and(|leads| *leads)",
        walks: "one byte, lowercased",
    },
    Allowance {
        file: "redact/credential.rs",
        code: "if !bytes.get(index..body)?.eq_ignore_ascii_case(prefix) {",
        walks: "a slice as long as a credential's prefix, from the constant table",
    },
    Allowance {
        file: "redact/credential.rs",
        code: "while index < credentials.len() {",
        walks: "the credential table, at compile time",
    },
    Allowance {
        file: "redact/credential.rs",
        code: "let lead = credentials[index].prefix.as_bytes()[0].to_ascii_lowercase();",
        walks: "the first byte of a credential's prefix, at compile time",
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
        code: "output.push_str(&text[start..index]);",
        walks: "the run a counted walk has just read, copied once",
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
