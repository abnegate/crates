//! How a file of some image type begins.

use std::ops::Range;

const RIFF: &[u8] = b"RIFF";

/// Where a RIFF container names its format, after its four-byte length.
const RIFF_FORMAT: Range<usize> = 8..12;

/// The bytes every file of one image type starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Signature {
    /// These bytes, first.
    Prefix(&'static [u8]),
    /// A RIFF container of this format.
    Riff(&'static [u8]),
}

impl Signature {
    /// Whether `bytes` begin as this signature says.
    pub(crate) fn starts(self, bytes: &[u8]) -> bool {
        match self {
            Self::Prefix(prefix) => bytes.starts_with(prefix),
            Self::Riff(format) => bytes.starts_with(RIFF) && bytes.get(RIFF_FORMAT) == Some(format),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Signature;

    #[test]
    fn a_prefix_matches_only_bytes_that_start_with_it() {
        let png = Signature::Prefix(b"\x89PNG\r\n\x1a\n");

        assert!(png.starts(b"\x89PNG\r\n\x1a\n\0\0"));
        assert!(!png.starts(b"\x89PNG"));
        assert!(!png.starts(b"GIF89a"));
    }

    #[test]
    fn a_riff_signature_reads_the_format_after_the_length() {
        let webp = Signature::Riff(b"WEBP");

        assert!(webp.starts(b"RIFF$\0\0\0WEBPVP8"));
        assert!(!webp.starts(b"RIFF$\0\0\0WAVEfmt"));
        assert!(!webp.starts(b"RIFF$\0\0\0WEB"));
        assert!(!webp.starts(b"RIFX$\0\0\0WEBPVP8"));
    }
}
