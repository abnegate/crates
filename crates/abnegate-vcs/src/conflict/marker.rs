/// The line a conflicted file opens each hunk with.
pub const OURS_MARKER: &str = "<<<<<<<";

/// The line diff3-style conflicts use to introduce the merge base.
pub const BASE_MARKER: &str = "|||||||";

/// The line separating the two sides of a conflict hunk.
pub const SPLIT_MARKER: &str = "=======";

/// The line a conflicted file closes each hunk with.
pub const THEIRS_MARKER: &str = ">>>>>>>";

/// Whether a file's text still carries the markers git wrote into it.
pub fn has_markers(text: &str) -> bool {
    text.lines()
        .any(|line| line.starts_with(OURS_MARKER) || line.starts_with(THEIRS_MARKER))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_are_recognised_only_at_the_start_of_a_line() {
        assert!(has_markers(
            "a\n<<<<<<< ours\nb\n=======\nc\n>>>>>>> theirs\n"
        ));
        assert!(!has_markers("a diff shows <<<<<<< inline\n"));
        assert!(!has_markers("plain text"));
    }
}
