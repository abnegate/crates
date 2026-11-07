//! The frame claude hands a subagent's report back to the main agent in.

use std::borrow::Cow;

const FRAME_START: &str = "[Subagent hand-back] ";
const FRAME_END: &str = " The report follows:";

/// What claude writes before every line of a report and of a note above its
/// frame, so that only the frame itself and what follows the report stand at
/// column zero.
const INDENT: &str = "  ";

/// How the lines claude writes below a report begin: the agent's id, then
/// where it worked and what it spent.
const TRAILERS: &[&str] = &["agentId: ", "<usage>"];

/// How the notes above a frame that are only for the main agent begin. Any
/// other note is kept, since it may say something the report leaves out: that
/// the subagent was cut off, say, which claude still reports as completed.
const INTERNAL_NOTES: &[&str] = &["<tools-used-before-summary>", "[harness: ", "[Harness note"];

/// What the result of a subagent's call says, as the subagent wrote it: the
/// frame, the indentation it added and the lines below it gone, and whatever
/// notes above it say more than the report does before it. A text of any other
/// shape is no frame, so it stands as it is.
pub(crate) fn report(text: &str) -> Cow<'_, str> {
    unframed(text).map_or(Cow::Borrowed(text), Cow::Owned)
}

fn unframed(text: &str) -> Option<String> {
    let mut lines = text.split('\n');
    let mut notes = Vec::new();
    let frame = loop {
        let line = lines.next()?;
        match line.strip_prefix(INDENT) {
            Some(note) => notes.push(note),
            None => break line,
        }
    };
    if !frame.starts_with(FRAME_START) || !frame.ends_with(FRAME_END) {
        return None;
    }

    let mut report = Vec::new();
    for line in lines {
        if let Some(line) = line.strip_prefix(INDENT) {
            report.push(line);
        } else if line.is_empty() {
            report.push(line);
        } else if TRAILERS.iter().any(|trailer| line.starts_with(trailer)) {
            break;
        } else {
            return None;
        }
    }

    let mut parts: Vec<String> = notes
        .split(|line| line.is_empty())
        .filter(|note| note.first().is_some_and(|line| !internal(line)))
        .map(|note| note.join("\n"))
        .collect();
    let report = report.join("\n");
    if !report.trim().is_empty() {
        parts.push(report);
    }
    Some(parts.join("\n\n"))
}

fn internal(note: &str) -> bool {
    INTERNAL_NOTES.iter().any(|start| note.starts_with(start))
}
