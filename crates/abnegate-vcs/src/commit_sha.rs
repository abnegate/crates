use crate::parse_error::ParseError;
use std::fmt;
use std::str::FromStr;

/// Hex digits in a SHA-1 object name.
const SHA1_LENGTH: usize = 40;

/// Hex digits in a SHA-256 object name.
const SHA256_LENGTH: usize = 64;

/// A full commit identifier in either of git's object formats, lowercased.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommitSha(String);

impl CommitSha {
    pub fn parse(value: &str) -> Result<Self, ParseError> {
        let trimmed = value.trim();
        let accepted = matches!(trimmed.len(), SHA1_LENGTH | SHA256_LENGTH)
            && trimmed.bytes().all(|byte| byte.is_ascii_hexdigit());
        match accepted {
            true => Ok(Self(trimmed.to_ascii_lowercase())),
            false => Err(ParseError::CommitSha(value.to_string())),
        }
    }

    /// The identifier as lowercase hex.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for CommitSha {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl AsRef<str> for CommitSha {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CommitSha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commit_identifier_is_forty_or_sixty_four_hex_digits() {
        let parsed = CommitSha::parse("A1B2C3D4E5F60718293A4B5C6D7E8F90A1B2C3D4").unwrap();
        assert_eq!(parsed.as_str(), "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4");
        assert_eq!(parsed.to_string(), parsed.as_str());

        let sha256 = "0123456789abcdef".repeat(4);
        assert_eq!(CommitSha::parse(&sha256).unwrap().as_str(), sha256);
        assert_eq!(
            CommitSha::parse(" a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4\n")
                .unwrap()
                .as_str(),
            "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4",
            "a line of git output parses"
        );

        for invalid in [
            "",
            "abc",
            "HEAD",
            "--output=/tmp/x",
            "z1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4",
            &"a".repeat(41),
            &"a".repeat(63),
        ] {
            assert!(
                CommitSha::parse(invalid).is_err(),
                "{invalid} is not a commit"
            );
        }
    }
}
