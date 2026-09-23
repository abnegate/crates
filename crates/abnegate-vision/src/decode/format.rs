//! Which decoder an image needs.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Jpeg,
    Png,
    WebP,
}

impl Format {
    /// The format `data` announces in its leading bytes.
    pub(crate) fn sniff(data: &[u8]) -> Option<Self> {
        match data {
            [0xff, 0xd8, 0xff, ..] => Some(Self::Jpeg),
            [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, ..] => Some(Self::Png),
            [
                b'R',
                b'I',
                b'F',
                b'F',
                _,
                _,
                _,
                _,
                b'W',
                b'E',
                b'B',
                b'P',
                ..,
            ] => Some(Self::WebP),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_supported_formats() {
        assert_eq!(Format::sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some(Format::Jpeg));
        assert_eq!(Format::sniff(b"\x89PNG\r\n\x1a\n\0"), Some(Format::Png));
        assert_eq!(Format::sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(Format::WebP));
        assert_eq!(Format::sniff(b"GIF89a..."), None);
    }
}
