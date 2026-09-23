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

/// Stands in for a line break that has been collapsed away.
///
/// A shell runs one command per line, so two lines joined by a space read as a
/// single command the reader was never shown. The break survives the collapse
/// as something they can see, and a call carrying the glyph itself has it
/// escaped, so every one a reader sees is a real break.
pub const LINE_BREAK: &str = " ⏎ ";

/// Collapse the blank space in `text`.
///
/// A command, a message body or a patch arrives with newlines and runs of
/// whitespace that would push the part worth reading off the card. Runs of
/// spaces and tabs within a line go and so do blank lines; the lines left keep
/// one `\n` between them, for a [`Preview`](super::Preview) to draw as
/// [`LINE_BREAK`], because what separates two commands is the part of a
/// preview a reader is deciding on.
///
/// Only `\n`, with the `\r` of a `\r\n` pair, breaks a line. Any other line
/// terminator or Unicode space is kept as itself for the preview to escape:
/// collapsing it into a space would show one line where the file holds two,
/// or an ordinary space where it holds something else.
pub(crate) fn collapse(text: &str) -> String {
    text.lines()
        .map(|line| {
            line.split([' ', '\t'])
                .filter(|word| !word.is_empty())
                .collect::<Vec<&str>>()
                .join(" ")
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<String>>()
        .join("\n")
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

    /// Blank space inside one line is noise that pushes the readable part off
    /// the card, so it still collapses.
    #[test]
    fn blank_space_inside_a_line_still_collapses() {
        assert_eq!(collapse("cargo    test   --all"), "cargo test --all");
        assert_eq!(collapse("  one\n\n\ntwo  "), "one\ntwo");
    }

    /// Only `\n`, or the `\r\n` pair around it, breaks a line, and only spaces
    /// and tabs collapse, so every other terminator and Unicode space is left
    /// for the preview to show as what it is.
    #[test]
    fn only_a_line_feed_breaks_a_line_and_only_spaces_and_tabs_collapse() {
        assert_eq!(collapse("one\r\ntwo"), "one\ntwo");
        assert_eq!(collapse("one\t \ttwo"), "one two");
        for character in [
            '\r', '\u{b}', '\u{c}', '\u{85}', '\u{a0}', '\u{2028}', '\u{3000}',
        ] {
            let text = format!("one{character}two");
            assert_eq!(collapse(&text), text, "{character:?}");
            assert_eq!(
                collapse(&format!("one{character}")),
                format!("one{character}"),
                "{character:?}"
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
