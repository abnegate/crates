use crate::commit_sha::CommitSha;

/// A git blob's content, whole.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Blob {
    /// The blob's object name.
    pub sha: CommitSha,
    /// Its size in bytes, which is the length of `content`.
    pub size: u64,
    /// Its bytes, decoded.
    pub content: Vec<u8>,
}

impl Blob {
    /// The blob `sha` names, holding `content`, its size the length of
    /// `content`.
    pub fn new(sha: CommitSha, content: Vec<u8>) -> Self {
        let size = u64::try_from(content.len()).unwrap_or(u64::MAX);
        Self { sha, size, content }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blob_is_as_large_as_its_content() {
        let blob = Blob::new(
            CommitSha::parse(&"b".repeat(40)).unwrap(),
            b"fn main() {}\n".to_vec(),
        );
        assert_eq!(blob.size, 13);
    }
}
