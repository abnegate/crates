/// The line a conflicted file opens each hunk with.
pub const OURS_MARKER: &str = "<<<<<<<";

/// The line diff3-style conflicts use to introduce the merge base.
pub const BASE_MARKER: &str = "|||||||";

/// The line separating the two sides of a conflict hunk.
pub const SPLIT_MARKER: &str = "=======";

/// The line a conflicted file closes each hunk with.
pub const THEIRS_MARKER: &str = ">>>>>>>";

/// One of the lines git writes around a conflict hunk: its marker, exactly,
/// then either the end of the line or a space and the label git gave it. A
/// longer run of the same character -- a Markdown underline, a banner comment
/// -- is text, not a marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Marker {
    Ours,
    Base,
    Split,
    Theirs,
}

impl Marker {
    pub(crate) fn parse(line: &str) -> Option<Self> {
        [
            (OURS_MARKER, Self::Ours),
            (BASE_MARKER, Self::Base),
            (SPLIT_MARKER, Self::Split),
            (THEIRS_MARKER, Self::Theirs),
        ]
        .into_iter()
        .find_map(|(marker, kind)| {
            line.strip_prefix(marker)
                .filter(|rest| rest.is_empty() || rest.starts_with(' '))
                .map(|_| kind)
        })
    }

    /// Whether this marker opens or closes a hunk, which no resolved file can
    /// carry; the split and base lines alone are ordinary text elsewhere.
    fn bounds_a_hunk(self) -> bool {
        matches!(self, Self::Ours | Self::Theirs)
    }
}

/// Whether a file's text still carries the markers git wrote into it.
pub fn has_markers(text: &str) -> bool {
    text.lines()
        .filter_map(Marker::parse)
        .any(Marker::bounds_a_hunk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_are_recognised_only_at_the_start_of_a_line() {
        assert!(has_markers(
            "a\n<<<<<<< ours\nb\n=======\nc\n>>>>>>> theirs\n"
        ));
        assert!(has_markers("<<<<<<<\nb\n>>>>>>>\n"));
        assert!(!has_markers("a diff shows <<<<<<< inline\n"));
        assert!(!has_markers("plain text"));
    }

    #[test]
    fn a_marker_is_exactly_seven_characters_then_a_space_or_the_end() {
        for (line, expected) in [
            ("<<<<<<<", Some(Marker::Ours)),
            ("<<<<<<< HEAD", Some(Marker::Ours)),
            ("|||||||", Some(Marker::Base)),
            ("||||||| base", Some(Marker::Base)),
            ("=======", Some(Marker::Split)),
            (">>>>>>> feature", Some(Marker::Theirs)),
            ("<<<<<<<<", None),
            ("<<<<<<<HEAD", None),
            ("========", None),
            ("======= heading", Some(Marker::Split)),
            ("||||||||", None),
            (">>>>>>>>>>", None),
            ("<<<<<<", None),
        ] {
            assert_eq!(Marker::parse(line), expected, "{line:?}");
        }
        assert!(!has_markers("Title\n========\n<<<<<<<< not a hunk\n"));
        assert!(
            !has_markers("Heading\n=======\n"),
            "a split line alone is a Markdown underline"
        );
    }
}
