use crate::work;

#[derive(Clone, Copy)]
pub(super) enum CharacterSet {
    /// `[A-Za-z0-9]`
    Alphanumeric,
    /// `[A-Za-z0-9_.:/+-]`
    Token,
    /// `[A-Za-z0-9_-]`
    Word,
    /// `[A-Za-z0-9+/_-]`
    Encoded,
}

impl CharacterSet {
    pub(super) fn contains(self, byte: u8) -> bool {
        byte.is_ascii_alphanumeric()
            || match self {
                Self::Alphanumeric => false,
                Self::Token => matches!(byte, b'_' | b'.' | b':' | b'/' | b'+' | b'-'),
                Self::Word => matches!(byte, b'_' | b'-'),
                Self::Encoded => matches!(byte, b'+' | b'/' | b'_' | b'-'),
            }
    }

    /// Where the run of this set's bytes that starts at `from` ends.
    pub(super) fn run(self, bytes: &[u8], from: usize) -> usize {
        work::run(bytes, from, |byte| self.contains(byte))
    }
}
