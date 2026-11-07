use crate::parse_error::ParseError;
use std::fmt;
use std::str::FromStr;

/// What separates one segment of a path from the next.
const SEPARATOR: char = '/';

/// A `.` as a URL may percent-encode it, compared in lowercase.
const ENCODED_DOT: &str = "%2e";

/// A path to a file or directory in a repository, whose segments each name
/// one entry and never step out of the directory before them.
///
/// A path with an empty segment, a `.` or `..` segment however a URL might
/// spell one, or any control character is refused: a URL parser acts on such
/// a segment instead of sending it, and could climb out of the repository.
/// Anything else, spaces and non-ASCII names included, is kept as written and
/// sent one percent-encoded segment at a time.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepositoryPath(String);

impl RepositoryPath {
    /// `value` without the slashes around it, or [`ParseError::RepositoryPath`]
    /// when that leaves nothing, a segment is empty or reads `.` or `..` (with
    /// any of its dots spelled `%2e`), or the path holds a control character.
    pub fn parse(value: &str) -> Result<Self, ParseError> {
        let path = value.trim_matches(SEPARATOR);
        let refused = path.is_empty()
            || path.chars().any(char::is_control)
            || path
                .split(SEPARATOR)
                .any(|segment| segment.is_empty() || dot(segment));

        match refused {
            true => Err(ParseError::RepositoryPath(value.to_string())),
            false => Ok(Self(path.to_string())),
        }
    }

    /// The path, with no slash at either end.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Each directory and file name along the path, in order.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split(SEPARATOR)
    }
}

/// Whether `segment` is `.` or `..` once every `%2e` in it reads as a dot.
fn dot(segment: &str) -> bool {
    matches!(
        segment
            .to_ascii_lowercase()
            .replace(ENCODED_DOT, ".")
            .as_str(),
        "." | ".."
    )
}

impl FromStr for RepositoryPath {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl AsRef<str> for RepositoryPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RepositoryPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn a_repository_path_is_split_into_the_segments_it_names() {
        for (value, path, segments) in [
            ("src/cart.ts", "src/cart.ts", vec!["src", "cart.ts"]),
            ("/src/a.rs/", "src/a.rs", vec!["src", "a.rs"]),
            (
                "docs/my file.md",
                "docs/my file.md",
                vec!["docs", "my file.md"],
            ),
            (
                "docs/caf\u{e9}/\u{65e5}\u{672c}.md",
                "docs/caf\u{e9}/\u{65e5}\u{672c}.md",
                vec!["docs", "caf\u{e9}", "\u{65e5}\u{672c}.md"],
            ),
            ("a%2Fb", "a%2Fb", vec!["a%2Fb"]),
            (
                ".github/workflows/ci.yml",
                ".github/workflows/ci.yml",
                vec![".github", "workflows", "ci.yml"],
            ),
            (
                "notes/.../..txt",
                "notes/.../..txt",
                vec!["notes", "...", "..txt"],
            ),
            ("README", "README", vec!["README"]),
        ] {
            let parsed = RepositoryPath::parse(value).expect(value);

            assert_eq!(parsed.as_str(), path, "{value:?}");
            assert_eq!(parsed.segments().collect::<Vec<_>>(), segments, "{value:?}");
            assert_eq!(parsed.to_string(), path);
            assert_eq!(parsed.as_ref(), path);
            assert_eq!(value.parse::<RepositoryPath>().unwrap(), parsed);
        }
    }

    #[test]
    fn a_repository_path_may_not_climb_or_hide_a_dot_segment() {
        for refused in [
            "..", ".", "a/../b", "a/..", "./a", "a//b", "", "/", "//", ".\t.", "a/.\n./b",
            "a/.\r/b", "a\0b", "a\u{7f}b", "a\u{85}b", "%2e%2e", "%2E%2e/b", "a/.%2E/b",
            "a/%2e./b", "%2E", "a/%2e",
        ] {
            match RepositoryPath::parse(refused) {
                Err(ParseError::RepositoryPath(value)) => assert_eq!(value, refused),
                other => panic!("{refused:?} must be refused as a repository path, got {other:?}"),
            }
        }

        let refusal = RepositoryPath::parse("a/../b").unwrap_err().to_string();
        assert!(refusal.contains("repository path"), "{refusal}");
        assert!(refusal.contains("a/../b"), "{refusal}");
    }

    #[test]
    fn every_segment_of_an_accepted_path_stays_one_segment_of_the_request() {
        let contents = Url::parse("https://api.github.com/repos/acme/project/contents").unwrap();
        for value in [
            "src/cart.ts",
            "docs/my file.md",
            "a%2Fb/c",
            "a\\..\\b",
            "q?x=1#y",
            "notes/.../..txt",
            "%252e%252e/x",
        ] {
            let path = RepositoryPath::parse(value).expect(value);
            let mut request = contents.clone();
            request.path_segments_mut().unwrap().extend(path.segments());

            let sent: Vec<&str> = request.path_segments().unwrap().collect();
            assert_eq!(
                sent.len(),
                4 + path.segments().count(),
                "{value:?} became {request}"
            );
            assert_eq!(
                &sent[..4],
                ["repos", "acme", "project", "contents"],
                "{request}"
            );
            assert!(
                request.query().is_none() && request.fragment().is_none(),
                "{request}"
            );
        }
    }
}
