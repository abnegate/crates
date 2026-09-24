use std::fmt;
use std::fmt::Write as _;

use crate::download::DownloadError;

const DIGEST_BYTES: usize = 32;
const HEXADECIMAL_RADIX: u32 = 16;

/// The SHA-256 a finished download must hash to.
///
/// HuggingFace publishes one for every LFS file as its `oid`, so a caller that
/// has the repository listing can hand it to
/// [`download_gguf`](crate::download::download_gguf) and have a spliced or
/// truncated file refused rather than installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checksum([u8; DIGEST_BYTES]);

impl Checksum {
    pub fn new(digest: [u8; DIGEST_BYTES]) -> Self {
        Self(digest)
    }

    /// Read a digest written as 64 hexadecimal characters, in either case.
    pub fn from_hexadecimal(hexadecimal: &str) -> Result<Self, DownloadError> {
        let invalid = || DownloadError::InvalidChecksum(hexadecimal.to_string());
        let characters = hexadecimal.trim().as_bytes();
        if characters.len() != DIGEST_BYTES * 2 {
            return Err(invalid());
        }

        let (pairs, _) = characters.as_chunks::<2>();
        let mut digest = [0_u8; DIGEST_BYTES];
        for (byte, [high, low]) in digest.iter_mut().zip(pairs) {
            let high = char::from(*high)
                .to_digit(HEXADECIMAL_RADIX)
                .ok_or_else(invalid)?;
            let low = char::from(*low)
                .to_digit(HEXADECIMAL_RADIX)
                .ok_or_else(invalid)?;
            *byte = u8::try_from(high * HEXADECIMAL_RADIX + low).map_err(|_| invalid())?;
        }
        Ok(Self(digest))
    }

    pub fn bytes(&self) -> &[u8; DIGEST_BYTES] {
        &self.0
    }
}

impl fmt::Display for Checksum {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut rendered = String::with_capacity(DIGEST_BYTES * 2);
        for byte in self.0 {
            let _ = write!(rendered, "{byte:02x}");
        }
        formatter.write_str(&rendered)
    }
}

#[cfg(test)]
mod tests {
    use super::Checksum;

    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn a_digest_round_trips_through_hexadecimal() {
        let checksum = Checksum::from_hexadecimal(EMPTY).unwrap();
        assert_eq!(checksum.to_string(), EMPTY);
        assert_eq!(checksum.bytes()[0], 0xe3);
        assert_eq!(
            Checksum::from_hexadecimal(&EMPTY.to_uppercase()).unwrap(),
            checksum
        );
    }

    #[test]
    fn a_malformed_digest_is_refused() {
        for malformed in ["", "e3b0", &EMPTY[1..], &format!("{}zz", &EMPTY[2..])] {
            assert!(
                Checksum::from_hexadecimal(malformed).is_err(),
                "{malformed}"
            );
        }
    }
}
