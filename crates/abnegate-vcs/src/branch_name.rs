use crate::parse_error::ParseError;
use std::fmt;
use std::str::FromStr;

/// Longest ref name git itself will accept without complaint.
const MAXIMUM_LENGTH: usize = 255;

/// Characters `git check-ref-format` refuses anywhere in a name, and the space
/// a command line would split on.
const FORBIDDEN_CHARACTERS: [char; 8] = ['~', '^', ':', '?', '*', '[', '\\', ' '];

/// The suffix git reserves for its own lock files, refused on any component.
const LOCK_SUFFIX: &str = ".lock";

/// The namespace a branch lives in.
const HEADS: &str = "refs/heads/";

/// Names git reserves for itself rather than for a branch.
const RESERVED: [&str; 2] = ["@", "HEAD"];

/// A branch name git accepts that no git command line can read as an option,
/// a revision expression or a pathspec.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BranchName(String);

impl BranchName {
    pub fn parse(value: &str) -> Result<Self, ParseError> {
        let refused = value.is_empty()
            || value.len() > MAXIMUM_LENGTH
            || RESERVED.contains(&value)
            || value.starts_with('-')
            || value.starts_with('/')
            || value.ends_with('/')
            || value.ends_with('.')
            || value.contains("..")
            || value.contains("@{")
            || value.contains("//")
            || value.contains(FORBIDDEN_CHARACTERS)
            || value.chars().any(char::is_control)
            || value.split('/').any(|component| {
                component.is_empty()
                    || component.starts_with('.')
                    || component.ends_with(LOCK_SUFFIX)
            });

        match refused {
            true => Err(ParseError::BranchName(value.to_string())),
            false => Ok(Self(value.to_string())),
        }
    }

    /// A name this crate spells itself. Every literal passed here is one the
    /// tests below parse, so none of them is ever refused.
    pub(crate) fn literal(value: &'static str) -> Self {
        debug_assert!(Self::parse(value).is_ok(), "{value:?} is not a branch name");
        Self(value.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The full ref this branch is stored under: `refs/heads/{name}`.
    pub fn reference(&self) -> String {
        format!("{HEADS}{}", self.0)
    }
}

impl FromStr for BranchName {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl AsRef<str> for BranchName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BranchName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_name_of_letters_digits_and_separators_is_accepted() {
        for name in [
            "main",
            "feature/one",
            "release/v1.2.3",
            "feature_branch",
            "user@feature",
            "a@b@c",
            "Feature/MyBranch",
            "origin/HEAD",
            "br\u{00e4}nch",
            "0123456789abcdef0123456789abcdef01234567",
        ] {
            assert_eq!(BranchName::parse(name).expect(name).as_str(), name);
        }
        assert_eq!(
            BranchName::parse("feature/one").unwrap().reference(),
            "refs/heads/feature/one"
        );
    }

    #[test]
    fn a_branch_name_may_not_be_an_option_a_revision_or_an_escape() {
        for invalid in [
            "",
            "@",
            "HEAD",
            "-force",
            "--upload-pack=evil",
            "/leading",
            "trailing/",
            "trailing.",
            "a..b",
            "a@{1}",
            "a//b",
            "a b",
            "a\tb",
            "a\nb",
            "a\u{7f}",
            "a^b",
            "HEAD~1",
            "a:b",
            "a*b",
            "a?b",
            "a[0]",
            "a\\b",
            ".",
            ".hidden",
            "dir/.hidden",
            "branch.lock",
            "dir.lock/branch",
        ] {
            assert!(
                BranchName::parse(invalid).is_err(),
                "{invalid:?} must not be accepted as a branch"
            );
        }
        assert!(
            BranchName::parse(&"a".repeat(MAXIMUM_LENGTH + 1)).is_err(),
            "a name longer than git accepts is not a branch"
        );
    }

    #[test]
    fn every_name_the_crate_spells_itself_is_a_branch_name() {
        for literal in ["main", "task"] {
            assert_eq!(
                BranchName::literal(literal),
                BranchName::parse(literal).unwrap()
            );
        }
    }

    #[test]
    fn a_refusal_names_what_arrived() {
        let refusal = BranchName::parse("--evil").unwrap_err().to_string();

        assert!(refusal.contains("--evil"), "{refusal}");
        assert!(refusal.contains("branch name"), "{refusal}");
    }

    #[test]
    fn a_branch_name_parses_from_a_string() {
        assert_eq!(
            "main".parse::<BranchName>().unwrap(),
            BranchName::parse("main").unwrap()
        );
        assert_eq!(BranchName::parse("main").unwrap().to_string(), "main");
    }
}
