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
        while let Some(found) = text[cursor..].find(OPENING) {
            let start = cursor + found;
            let after = start + OPENING.len();
            let Some(length) = text[after..].find(CLOSING) else {
                break;
            };
            let end = after + length;
            let expression = &text[after..end];
            let (name, default) = match expression.split_once(DEFAULT) {
                Some((name, default)) => (name, Some(default)),
                None => (expression, None),
            };
            if !variable(name) {
                cursor = start + 1;
                continue;
            }
            if start > literal {
                segments.push(Self::Literal(&text[literal..start]));
            }
            segments.push(Self::Reference {
                written: &text[start..=end],
                name,
                default,
            });
            cursor = end + 1;
            literal = cursor;
        }
        if literal < text.len() {
            segments.push(Self::Literal(&text[literal..]));
        }
        segments
    }
}

/// Whether `name` is one a shell variable can have.
fn variable(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

#[cfg(test)]
mod tests {
    use super::Segment;

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
}
