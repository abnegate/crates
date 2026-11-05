use crate::commit_sha::CommitSha;
use crate::parse_error::ParseError;
use serde::Deserialize;
use serde::Serialize;
use std::fmt;
use std::str::FromStr;

/// Hex digits in a SHA-1 object name.
const SHA1_LENGTH: usize = 40;

/// Hex digits in a SHA-256 object name.
const SHA256_LENGTH: usize = 64;

/// A full git object name, of a tree, a blob, a commit or a tag, in either of
/// git's object formats, lowercased.
///
/// It is held to the rules a [`CommitSha`] is, and a commit's identifier
/// names an object too, so [`ObjectId::from`] turns one into the other: a
/// commit's identifier names its tree to GitHub's trees API. It is written as
/// its hex string, and read back through [`ObjectId::parse`], so a stored
/// value this crate does not accept is an error rather than an object.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ObjectId(String);

impl ObjectId {
    /// Accept a 40-digit SHA-1 or 64-digit SHA-256 hex object name, trimmed
    /// of surrounding whitespace and lowercased, or fail with
    /// [`ParseError::ObjectId`]. An abbreviated name is refused.
    pub fn parse(value: &str) -> Result<Self, ParseError> {
        named(value)
            .map(Self)
            .ok_or_else(|| ParseError::ObjectId(value.to_string()))
    }

    /// The object name as lowercase hex.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `value`, trimmed and lowercased, when it is a full object name in either
/// of git's formats.
pub(crate) fn named(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let accepted = matches!(trimmed.len(), SHA1_LENGTH | SHA256_LENGTH)
        && trimmed.bytes().all(|byte| byte.is_ascii_hexdigit());
    accepted.then(|| trimmed.to_ascii_lowercase())
}

impl From<&CommitSha> for ObjectId {
    fn from(commit: &CommitSha) -> Self {
        Self(commit.as_str().to_string())
    }
}

impl From<CommitSha> for ObjectId {
    fn from(commit: CommitSha) -> Self {
        Self(commit.into())
    }
}

impl FromStr for ObjectId {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl TryFrom<String> for ObjectId {
    type Error = ParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ObjectId> for String {
    fn from(object: ObjectId) -> Self {
        object.0
    }
}

impl AsRef<str> for ObjectId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_object_name_is_forty_or_sixty_four_hex_digits() {
        let parsed = ObjectId::parse(" A1B2C3D4E5F60718293A4B5C6D7E8F90A1B2C3D4\n").unwrap();
        assert_eq!(parsed.as_str(), "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4");
        assert_eq!(parsed.to_string(), parsed.as_str());

        let sha256 = "0123456789abcdef".repeat(4);
        assert_eq!(ObjectId::parse(&sha256).unwrap().as_str(), sha256);

        for invalid in [
            "",
            "abc",
            "HEAD",
            "--output=/tmp/x",
            "z1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4",
            &"a".repeat(39),
            &"a".repeat(41),
            &"a".repeat(63),
            &"a".repeat(65),
        ] {
            assert_eq!(
                ObjectId::parse(invalid),
                Err(ParseError::ObjectId(invalid.to_string())),
                "{invalid} is not an object name"
            );
        }
    }

    #[test]
    fn a_commit_names_the_same_object() {
        let commit = CommitSha::parse(&"C".repeat(40)).unwrap();

        assert_eq!(ObjectId::from(&commit).as_str(), commit.as_str());
        assert_eq!(ObjectId::from(commit.clone()).as_str(), commit.as_str());
    }

    #[test]
    fn an_object_is_written_as_its_hex_and_read_back_only_if_it_parses() {
        let object = ObjectId::parse(&"B".repeat(64)).unwrap();

        let written = serde_json::to_value(&object).unwrap();

        assert_eq!(written, serde_json::json!("b".repeat(64)));
        assert_eq!(serde_json::from_value::<ObjectId>(written).unwrap(), object);
        assert!(serde_json::from_value::<ObjectId>(serde_json::json!("HEAD")).is_err());
    }
}
