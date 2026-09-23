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

/// Longest a rendered approval preview may run.
///
/// The reader is deciding, not reading. A preview past a screenful is one
/// nobody finishes, and an unfinished preview is worse than none.
pub const MAX_PREVIEW_CHARACTERS: usize = 400;

/// Stands in for a line break that has been collapsed away.
///
/// A shell runs one command per line, so two lines joined by a space read as a
/// single command the reader was never shown. The break survives the collapse
/// as something they can see.
pub const LINE_BREAK: &str = " ⏎ ";

/// Collapse `text` onto one line and cut it to `max_characters`.
///
/// A command, a message body or a patch arrives with newlines and runs of
/// whitespace that would push the part worth reading off the card. Runs of
/// blank space within a line go; a line break becomes [`LINE_BREAK`], because
/// what separates two commands is the part of a preview a reader is deciding
/// on.
pub fn excerpt(text: &str, max_characters: usize) -> String {
    let collapsed = text
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<&str>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<String>>()
        .join(LINE_BREAK);
    match collapsed.char_indices().nth(max_characters) {
        Some((index, _)) => format!("{}…", &collapsed[..index]),
        None => collapsed,
    }
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
        assert_eq!(excerpt("cargo    test   --all", 400), "cargo test --all");
        assert_eq!(
            excerpt("  one\n\n\ntwo  ", 400),
            format!("one{LINE_BREAK}two")
        );
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
