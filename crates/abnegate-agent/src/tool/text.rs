use std::borrow::Cow;
use std::iter::repeat_n;

/// Budget a tool spends on output it pages or trims for itself: a `read_file`
/// page, a captured command log.
pub const MAX_TOOL_OUTPUT_CHARACTERS: usize = 8_000;

/// Headroom between a tool's own budget and the transcript cap, for the
/// framing a tool wraps around its output: a pagination footer, an exit-code
/// line, the `Error: ` prefix.
const TOOL_FRAMING_CHARACTERS: usize = 1_000;

/// Last-resort cap on tool text stored in the chat transcript.
///
/// Sits above [`MAX_TOOL_OUTPUT_CHARACTERS`] and the framing around it, so a tool
/// that stayed inside its own budget is never cut here. Setting the two equal
/// meant a full page plus its footer overflowed by a few characters and lost
/// its middle to a second trim, which is the double cut this exists to avoid.
pub const MAX_TOOL_MESSAGE_CHARACTERS: usize = MAX_TOOL_OUTPUT_CHARACTERS + TOOL_FRAMING_CHARACTERS;

/// What [`ToolResult::to_message`](super::ToolResult::to_message) puts in
/// front of a failure.
///
/// A tool trimming to a caller's cap has to reserve this, or the message the
/// model reads is longer than the cap it asked for.
pub(crate) const ERROR_PREFIX: &str = "Error: ";

/// Longest a [`Preview`](super::Preview) runs before its middle is hidden.
///
/// The reader is deciding, not reading, and a preview past a screenful is one
/// nobody finishes. What does not fit is cut from the middle, where the cut is
/// marked and counted and the preview flagged, never silently from the end.
pub const MAX_PREVIEW_CHARACTERS: usize = 400;

/// How a [`Preview`](super::Preview) draws a line break.
///
/// A shell runs one command per line, so two lines joined by a space read as a
/// single command the reader was never shown. The break is drawn as something
/// they can see, and a call carrying the glyph itself has it escaped, so every
/// one a reader sees is a real break.
pub const LINE_BREAK: &str = " ⏎ ";

/// What opens and closes a span [`quote`] draws.
const QUOTE: char = '"';

/// `text` between double quotes, for a preview to show content in: the text
/// a write puts in a file, or what an edit takes out and puts in.
///
/// Every `"` in `text` is shown behind a backslash, and the backslashes
/// straight before a `"` or the closing quote are shown doubled, so a quote
/// is part of the content exactly when an odd number of backslashes precede
/// it. Content that could close its own span could otherwise draw the next
/// one: a replacement, or the end of a write it does not end. Every other
/// backslash is shown as it is, so a line the content continues with a `\`
/// still reads as continued.
pub(crate) fn quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push(QUOTE);
    let mut backslashes = 0;
    for character in text.chars() {
        match character {
            '\\' => backslashes += 1,
            QUOTE => {
                quoted.extend(repeat_n('\\', backslashes * 2 + 1));
                quoted.push(QUOTE);
                backslashes = 0;
            }
            _ => {
                quoted.extend(repeat_n('\\', backslashes));
                quoted.push(character);
                backslashes = 0;
            }
        }
    }
    quoted.extend(repeat_n('\\', backslashes * 2));
    quoted.push(QUOTE);
    quoted
}

/// `text` as one word of a command line, the way a shell would read it back.
///
/// As it is when every character is one a shell takes as itself anywhere in
/// a word, and otherwise between single quotes, a `'` it holds written as
/// `'\''`. Words joined by a space lost where one ended and the next began,
/// so `-name '*.rs -delete'`, one pattern, read as a pattern and a
/// `-delete`, and a path holding a space as two.
pub(crate) fn word(text: &str) -> Cow<'_, str> {
    if !text.is_empty() && text.chars().all(literal) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(format!("'{}'", text.replace('\'', r"'\''")))
}

/// Whether a shell takes `character` as itself wherever it falls in a word.
fn literal(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || matches!(
            character,
            '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '_' | '-'
        )
}

fn trim_marker(dropped: usize) -> String {
    format!("\n\n[… {dropped} characters trimmed …]\n\n")
}

/// Cap `text` at `max_characters`, dropping the middle rather than the tail.
///
/// The error a build was run for is usually at the end, so a cut that keeps
/// only the head throws away the reason for the call. The marker is paid for
/// out of the budget, which makes a second pass over already-trimmed text a
/// no-op.
pub(crate) fn trim_middle(text: &str, max_characters: usize) -> String {
    let characters: Vec<char> = text.chars().collect();
    if characters.len() <= max_characters {
        return text.to_string();
    }
    // The widest the marker can get, so head + marker + tail always fits.
    let reserved = trim_marker(characters.len()).chars().count();
    let kept = max_characters.saturating_sub(reserved);
    let head = kept / 2;
    let tail = kept - head;
    format!(
        "{}{}{}",
        characters[..head].iter().collect::<String>(),
        trim_marker(characters.len() - kept),
        characters[characters.len() - tail..]
            .iter()
            .collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quoted_span_escapes_every_quote_it_holds_and_the_backslashes_before_one() {
        assert_eq!(quote("plain"), r#""plain""#);
        assert_eq!(quote(""), r#""""#);
        assert_eq!(quote(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote(r#"a\"b"#), r#""a\\\"b""#);
        assert_eq!(quote(r"C:\dir\"), r#""C:\dir\\""#);
        assert_eq!(
            quote("echo first \\\necho second"),
            "\"echo first \\\necho second\""
        );
    }

    #[test]
    fn a_word_is_quoted_whenever_a_shell_would_not_read_it_as_itself() {
        assert_eq!(word("src/lib.rs"), "src/lib.rs");
        assert_eq!(word("--features=a,b"), "--features=a,b");
        assert_eq!(word(""), "''");
        assert_eq!(word("*.rs -delete"), "'*.rs -delete'");
        assert_eq!(word("my notes.txt"), "'my notes.txt'");
        assert_eq!(word("it's"), r"'it'\''s'");
        assert_eq!(word("~/tmp"), "'~/tmp'");
        assert_eq!(word("a\"b"), "'a\"b'");
        for metacharacter in [
            ' ', '\t', '\n', '\\', '$', '`', ';', '&', '|', '<', '>', '(', ')', '*', '?', '[', ']',
            '{', '}', '#', '!', '~', '"', '\'',
        ] {
            let text = format!("a{metacharacter}b");
            assert!(
                word(&text).starts_with('\''),
                "{metacharacter:?}: {}",
                word(&text)
            );
        }
    }

    #[test]
    fn trim_middle_keeps_head_and_tail_inside_the_budget() {
        let text = format!("HEAD{}TAIL", "x".repeat(40_000));
        let trimmed = trim_middle(&text, MAX_TOOL_MESSAGE_CHARACTERS);
        assert!(trimmed.starts_with("HEAD"), "{trimmed}");
        assert!(trimmed.ends_with("TAIL"), "{trimmed}");
        assert!(trimmed.contains("characters trimmed"), "{trimmed}");
        assert!(trimmed.chars().count() <= MAX_TOOL_MESSAGE_CHARACTERS);
        assert_eq!(trim_middle("hello", MAX_TOOL_MESSAGE_CHARACTERS), "hello");
        assert_eq!(
            trim_middle(&trimmed, MAX_TOOL_MESSAGE_CHARACTERS),
            trimmed,
            "trimming an already trimmed string must not cut it again"
        );
    }
}
