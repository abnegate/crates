#[cfg(feature = "github")]
use crate::parse_error::ParseError;
use crate::remote_branch_name::RemoteBranchName;

/// The name `git tag` refuses, since git reads it as the checked-out commit.
const HEAD: &str = "HEAD";

/// A tag name git accepts: one `git check-ref-format` accepts under
/// `refs/tags/`, and that `git tag` does not refuse for being `HEAD` or for
/// being led by `-`.
///
/// Every branch name git accepts is one too, so these are the rules any name
/// of a ref under `refs/` is held to before it is sent as a path segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TagName(String);

impl TagName {
    /// Accept `value` as it is, untrimmed, or fail with [`ParseError::Tag`]
    /// when git would refuse it as a tag.
    #[cfg(feature = "github")]
    pub(crate) fn parse(value: &str) -> Result<Self, ParseError> {
        match Self::refuses(value) {
            true => Err(ParseError::Tag(value.to_string())),
            false => Ok(Self(value.to_string())),
        }
    }

    /// Whether git refuses `value` as a tag: empty or `HEAD`, led by `-` or
    /// `/`, ended by `/` or `.`, holding `..`, `@{`, `//`, an ASCII control
    /// character or any of `~^:?*[\` and the space, or with a component that
    /// starts with `.` or ends in `.lock`.
    pub(crate) fn refuses(value: &str) -> bool {
        RemoteBranchName::refuses(value) || value == HEAD || value.starts_with('-')
    }

    /// The name as git spells it.
    #[cfg(feature = "github")]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_git_accepts_is_a_tag_name() {
        for name in [
            "v1.2.3",
            "pkg/v1.2.3",
            "v1.0.0+build",
            "+v1",
            "@",
            "a@b",
            "refs/x",
            "t\u{0085}",
            &format!("pkg/{}", "a".repeat(250)),
        ] {
            assert!(!TagName::refuses(name), "{name:?} is a tag git accepts");
        }
    }

    #[test]
    fn a_tag_git_refuses_is_no_tag_name() {
        for name in [
            "",
            "HEAD",
            "-v1",
            "/v1",
            "v1/",
            "v1.",
            "v1..2",
            "v1@{1}",
            "a//b",
            "v 1",
            "v1~1",
            "v1^",
            "a:b",
            "a?b",
            "a*b",
            "a[0]",
            "a\\b",
            "a\tb",
            "a\u{7f}",
            ".v1",
            "pkg/.v1",
            "v1.lock",
            "v1.lock/x",
        ] {
            assert!(
                TagName::refuses(name),
                "{name:?} must not be accepted as a tag"
            );
        }
    }
}
