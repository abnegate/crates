use crate::conflict::ConflictError;
use crate::conflict::ConflictResult;

/// Longest ref name git itself will accept without complaint.
const MAX_BRANCH_LENGTH: usize = 255;

/// A branch name git will accept and a shell cannot reinterpret.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BranchName(String);

impl BranchName {
    pub fn parse(value: &str) -> ConflictResult<Self> {
        let refused = value.is_empty()
            || value.len() > MAX_BRANCH_LENGTH
            || value.starts_with('-')
            || value.starts_with('/')
            || value.ends_with('/')
            || value.ends_with('.')
            || value.contains("..")
            || value.contains("@{")
            || value.contains("//")
            || value.contains(['~', '^', ':', '?', '*', '[', '\\', ' '])
            || value.chars().any(|character| character.is_control())
            || value
                .split('/')
                .any(|part| part.is_empty() || part.starts_with('.'));

        if refused {
            return Err(ConflictError::InvalidBranch(value.to_string()));
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BranchName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_name_may_not_be_an_option_or_an_escape() {
        assert!(BranchName::parse("feature/one").is_ok());
        for invalid in [
            "",
            "-force",
            "/leading",
            "trailing/",
            "a..b",
            "a@{1}",
            "a//b",
            "a b",
            "a^b",
            "a:b",
            "a*b",
            "a\\b",
            ".hidden",
            "dir/.hidden",
        ] {
            assert!(
                BranchName::parse(invalid).is_err(),
                "{invalid:?} must not be accepted as a branch"
            );
        }
        assert!(
            BranchName::parse(&"a".repeat(MAX_BRANCH_LENGTH + 1)).is_err(),
            "a name longer than git accepts is not a branch"
        );
    }
}
