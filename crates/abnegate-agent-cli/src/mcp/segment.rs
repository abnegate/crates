pub(crate) const OPENING: &str = "${";
pub(crate) const CLOSING: char = '}';
const DEFAULT: &str = ":-";

/// A run of literal text, or one `${VAR}` or `${VAR:-default}` reference as
/// `written`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Segment<'text> {
    Literal(&'text str),
    Reference {
        written: &'text str,
        name: &'text str,
        default: Option<&'text str>,
    },
}

impl<'text> Segment<'text> {
    /// `text` as literal runs and references, in order, read as Claude Code
    /// reads them: a `${` that does not open a reference to a variable is
    /// literal text, and reading goes on just past its `$`, so a reference
    /// inside it still counts; one never closed is literal to the end.
    pub(crate) fn split(text: &'text str) -> Vec<Self> {
        let mut segments = Vec::new();
        let mut literal = 0;
        let mut cursor = 0;
        let mut closing = 0;
        while let Some(found) = text[cursor..].find(OPENING) {
            let start = cursor + found;
            let after = start + OPENING.len();
            if closing < after {
                let Some(length) = text[after..].find(CLOSING) else {
                    break;
                };
                closing = after + length;
            }
            let Some((name, default)) = Self::read(&text[after..closing]) else {
                cursor = start + 1;
                continue;
            };
            if start > literal {
                segments.push(Self::Literal(&text[literal..start]));
            }
            segments.push(Self::Reference {
                written: &text[start..=closing],
                name,
                default,
            });
            cursor = closing + 1;
            literal = cursor;
        }
        if literal < text.len() {
            segments.push(Self::Literal(&text[literal..]));
        }
        segments
    }

    /// The name and default in `expression`, the text between a `${` and the
    /// first `}` after it, when it reads as a variable's name alone or one
    /// followed by `:-` and its default.
    fn read(expression: &'text str) -> Option<(&'text str, Option<&'text str>)> {
        let length = expression
            .find(|character: char| !word(character))
            .unwrap_or(expression.len());
        let (name, rest) = expression.split_at(length);
        let default = match rest {
            "" => None,
            rest => Some(rest.strip_prefix(DEFAULT)?),
        };
        name.starts_with(|first: char| first.is_ascii_alphabetic() || first == '_')
            .then_some((name, default))
    }
}

/// Whether `character` can stand in a shell variable's name.
fn word(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::OPENING;
    use super::Segment;

    const PATIENCE: Duration = Duration::from_secs(30);

    #[test]
    fn text_splits_into_literal_runs_and_references() {
        assert_eq!(
            Segment::split("Bearer ${TOKEN}; id=${ID:-anonymous}"),
            [
                Segment::Literal("Bearer "),
                Segment::Reference {
                    written: "${TOKEN}",
                    name: "TOKEN",
                    default: None,
                },
                Segment::Literal("; id="),
                Segment::Reference {
                    written: "${ID:-anonymous}",
                    name: "ID",
                    default: Some("anonymous"),
                },
            ]
        );
    }

    #[test]
    fn a_dollar_brace_that_names_no_variable_is_literal_text() {
        assert_eq!(
            Segment::split("${1BAD} ${} ${A B} ${"),
            [Segment::Literal("${1BAD} ${} ${A B} ${")]
        );
        assert_eq!(Segment::split(""), []);
    }

    /// Claude Code's pattern is `\$\{([A-Za-z_][A-Za-z0-9_]*(?::-[^}]*)?)\}`:
    /// a name, then `}` or `:-` and a default that runs to the first `}`.
    #[test]
    fn a_reference_is_a_name_then_a_closing_brace_or_a_default() {
        assert_eq!(
            Segment::split("${A:-${B}${C:x}${_D}"),
            [
                Segment::Reference {
                    written: "${A:-${B}",
                    name: "A",
                    default: Some("${B"),
                },
                Segment::Literal("${C:x}"),
                Segment::Reference {
                    written: "${_D}",
                    name: "_D",
                    default: None,
                },
            ]
        );
        assert_eq!(
            Segment::split("${A ${B:-}"),
            [
                Segment::Literal("${A "),
                Segment::Reference {
                    written: "${B:-}",
                    name: "B",
                    default: Some(""),
                },
            ]
        );
    }

    /// A name is ASCII, as the CLI's pattern has it, and anything else, a
    /// default included, may be any text, so a value is cut only at the
    /// ASCII `${`, `:-` and `}` around a reference.
    #[test]
    fn a_reference_reads_among_and_holds_text_that_is_not_ascii() {
        assert_eq!(
            Segment::split("é${A}ü"),
            [
                Segment::Literal("é"),
                Segment::Reference {
                    written: "${A}",
                    name: "A",
                    default: None,
                },
                Segment::Literal("ü"),
            ]
        );
        assert_eq!(Segment::split("${Aé}"), [Segment::Literal("${Aé}")]);
        assert_eq!(
            Segment::split("${A:-é}"),
            [Segment::Reference {
                written: "${A:-é}",
                name: "A",
                default: Some("é"),
            }]
        );
        assert_eq!(Segment::split("${A:-x"), [Segment::Literal("${A:-x")]);
        assert_eq!(
            Segment::split("${A:-x}y}"),
            [
                Segment::Reference {
                    written: "${A:-x}",
                    name: "A",
                    default: Some("x"),
                },
                Segment::Literal("y}"),
            ]
        );
    }

    /// Every `${` in a value of many and one `}` used to send the reader on
    /// to that `}` and back, so a value a few megabytes long held a run up
    /// for minutes; one pass reads it in well under a second. The reader runs
    /// on a thread of its own, so a regression fails once its patience runs
    /// out rather than whenever it finishes.
    #[test]
    fn a_long_value_of_openings_before_one_closing_brace_is_read_in_one_pass() {
        let count = 1 << 21;
        for value in [
            format!("{}}}", OPENING.repeat(count)),
            format!("{}}}", "${A ".repeat(count)),
        ] {
            let (sender, receiver) = mpsc::channel();
            let text = value.clone();
            thread::spawn(move || {
                let read = Segment::split(&text) == [Segment::Literal(text.as_str())];
                let _ = sender.send(read);
            });

            let read = receiver
                .recv_timeout(PATIENCE)
                .expect("a value read in one pass");

            assert!(read, "a value of {} bytes", value.len());
        }
    }
}
