//! Content hash for incremental indexing.

use sha2::Digest;
use sha2::Sha256;

/// Lowercase SHA-256 of `blob`, used to skip unchanged files.
pub fn file_hash(blob: &str) -> String {
    hex::encode(Sha256::digest(blob.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_and_lowercase() {
        let digest = file_hash("hello");
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
        );
        assert_eq!(digest, file_hash("hello"));
        assert_ne!(digest, file_hash("Hello"));
    }
}
