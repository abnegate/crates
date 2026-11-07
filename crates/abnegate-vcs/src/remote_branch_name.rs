use crate::branch_name::BranchName;
use crate::parse_error::ParseError;
use std::fmt;
use std::str::FromStr;

/// Characters `git check-ref-format` refuses anywhere in a ref name, the
/// space among them.
const FORBIDDEN_CHARACTERS: &[char] = &['~', '^', ':', '?', '*', '[', '\\', ' '];

/// The suffix git reserves for its own lock files, refused on any component.
const LOCK_SUFFIX: &str = ".lock";

/// Git's shorthand for `HEAD`, a revision expression rather than a name,
/// though git accepts it under `refs/`.
#[cfg(any(feature = "github", feature = "gitlab"))]
pub(crate) const CURRENT: &str = "@";

/// A branch name as a host such as GitHub reports it: any name
/// `git check-ref-format` accepts under `refs/heads/`.
///
/// A host keeps branches no local `git branch` would create, such as `+release`
/// or `-dash`, so this holds more than a [`BranchName`] does. It is only ever
/// read and shown; [`Self::to_branch_name`] is the one way onto a git command
/// line, and it refuses any name a command line could misread.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RemoteBranchName(String);

impl RemoteBranchName {
    /// Accept `value` as it is, untrimmed, or fail with
    /// [`ParseError::BranchName`] when `git check-ref-format` would refuse it
    /// under `refs/heads/`: empty, led by `/`, ended by `/` or `.`, holding
    /// `..`, `@{`, `//`, an ASCII control character or any of `~^:?*[\` and
    /// the space, or with a component that starts with `.` or ends in `.lock`.
    pub fn parse(value: &str) -> Result<Self, ParseError> {
        match Self::refuses(value) {
            true => Err(ParseError::BranchName(value.to_string())),
            false => Ok(Self(value.to_string())),
        }
    }

    /// Whether `git check-ref-format` refuses `value` as a name under
    /// `refs/heads/` or `refs/tags/`.
    pub(crate) fn refuses(value: &str) -> bool {
        value.is_empty()
            || value.starts_with('/')
            || value.ends_with(['/', '.'])
            || value.contains("..")
            || value.contains("@{")
            || value.contains("//")
            || value.contains(FORBIDDEN_CHARACTERS)
            || value.chars().any(|character| character.is_ascii_control())
            || value
                .split('/')
                .any(|component| component.starts_with('.') || component.ends_with(LOCK_SUFFIX))
    }

    /// Whether `value` is refused as a ref a host is asked to read at, which
    /// travels percent-encoded and never reaches a command line: a name
    /// [`Self::refuses`] refuses, or `@`, git's shorthand for `HEAD`.
    #[cfg(any(feature = "github", feature = "gitlab"))]
    pub(crate) fn refuses_reference(value: &str) -> bool {
        value == CURRENT || Self::refuses(value)
    }

    /// The name as the host spells it.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The same name as a [`BranchName`], safe to put on a git command line,
    /// or [`ParseError::BranchName`] when [`BranchName::parse`] refuses it,
    /// as it does `+release`, `-dash` and `HEAD`.
    pub fn to_branch_name(&self) -> Result<BranchName, ParseError> {
        BranchName::parse(&self.0)
    }
}

impl From<BranchName> for RemoteBranchName {
    fn from(branch: BranchName) -> Self {
        Self(branch.as_str().to_string())
    }
}

impl FromStr for RemoteBranchName {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl AsRef<str> for RemoteBranchName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RemoteBranchName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_a_command_line_would_misread_is_still_a_remote_branch_name() {
        for name in [
            "main",
            "feature/one",
            "+release",
            "-dash",
            "--upload-pack=evil",
            "HEAD",
            "@",
            "a@b",
            "br\u{00e4}nch",
            "t\u{0085}",
            &"a".repeat(300),
        ] {
            assert_eq!(RemoteBranchName::parse(name).expect(name).as_str(), name);
        }
    }

    #[test]
    fn a_name_git_refuses_under_refs_heads_is_no_remote_branch_name() {
        for name in [
            "",
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
            "a~1",
            "a^b",
            "a:b",
            "a?b",
            "a*b",
            "a[0]",
            "a\\b",
            ".hidden",
            "dir/.hidden",
            "branch.lock",
            "dir.lock/branch",
        ] {
            assert!(
                RemoteBranchName::parse(name).is_err(),
                "{name:?} must not be accepted as a remote branch"
            );
        }
    }

    #[cfg(any(feature = "github", feature = "gitlab"))]
    #[test]
    fn a_reference_is_any_remote_branch_name_other_than_git_s_shorthand() {
        for (value, refused) in [
            ("HEAD", false),
            ("main", false),
            ("feature/x", false),
            ("-dash", false),
            ("--upload-pack=evil", false),
            ("+release", false),
            ("@", true),
            ("a..b", true),
            ("", true),
        ] {
            assert_eq!(
                RemoteBranchName::refuses_reference(value),
                refused,
                "{value:?} refused as a reference"
            );
        }
    }

    #[test]
    fn a_name_a_command_line_would_misread_is_no_branch_name() {
        for name in [
            "+release",
            "-dash",
            "--upload-pack=evil",
            "HEAD",
            "@",
            "t\u{0085}",
        ] {
            let remote = RemoteBranchName::parse(name).unwrap();

            assert_eq!(
                remote.to_branch_name(),
                Err(ParseError::BranchName(name.to_string())),
                "{name:?} reached a command line"
            );
        }
    }

    #[test]
    fn a_name_safe_on_a_command_line_converts_to_the_same_branch_name() {
        for name in ["main", "feature/one", "release/v1.2.3"] {
            let remote = RemoteBranchName::parse(name).unwrap();

            assert_eq!(remote.to_branch_name(), BranchName::parse(name));
        }
    }

    #[test]
    fn every_branch_name_is_a_remote_branch_name() {
        let branch = BranchName::parse("feature/one").unwrap();

        assert_eq!(
            RemoteBranchName::from(branch.clone()),
            RemoteBranchName::parse(branch.as_str()).unwrap()
        );
    }

    #[test]
    fn a_remote_branch_name_parses_from_and_renders_as_a_string() {
        let name: RemoteBranchName = "+release".parse().unwrap();

        assert_eq!(name, RemoteBranchName::parse("+release").unwrap());
        assert_eq!(name.to_string(), "+release");
        assert_eq!(name.as_ref(), "+release");
    }

    #[test]
    fn a_refusal_names_what_arrived() {
        let refusal = RemoteBranchName::parse("a..b").unwrap_err().to_string();

        assert!(refusal.contains("a..b"), "{refusal}");
        assert!(refusal.contains("branch name"), "{refusal}");
    }
}
