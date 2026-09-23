use crate::conflict::ConflictError;
use crate::conflict::ConflictResult;

/// A full commit identifier, lowercased.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommitSha(String);

impl CommitSha {
    pub fn parse(value: &str) -> ConflictResult<Self> {
        let trimmed = value.trim();
        if trimmed.len() != 40 || !trimmed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ConflictError::InvalidCommit(value.to_string()));
        }
        Ok(Self(trimmed.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CommitSha {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commit_identifier_must_be_forty_hex_digits() {
        let parsed = CommitSha::parse("A1B2C3D4E5F60718293A4B5C6D7E8F90A1B2C3D4").unwrap();
        assert_eq!(parsed.as_str(), "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4");
        assert_eq!(parsed.to_string(), parsed.as_str());

        for invalid in ["", "abc", "z1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4"] {
            assert!(
                CommitSha::parse(invalid).is_err(),
                "{invalid} is not a commit"
            );
        }
    }
}
