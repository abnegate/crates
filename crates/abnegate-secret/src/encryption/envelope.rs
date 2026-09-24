const OPENING: &str = "ENC[v";
const SEPARATOR: char = ':';
const CLOSING: &str = "]";

/// A stored value that opens an envelope: `ENC[v`, a version of one or more
/// digits, then `:`, whether or not what follows is well formed.
pub(super) struct Envelope<'a> {
    version: &'a str,
    rest: &'a str,
}

impl<'a> Envelope<'a> {
    /// The version every envelope is sealed in, and the only one that opens.
    pub(super) const VERSION: &'static str = "1";

    /// The envelope `value` opens, or `None` when it opens none.
    pub(super) fn open(value: &'a str) -> Option<Self> {
        let rest = value.strip_prefix(OPENING)?;
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let (version, rest) = rest.split_at(digits);
        let rest = rest.strip_prefix(SEPARATOR)?;
        (!version.is_empty()).then_some(Self { version, rest })
    }

    /// `encoded` wrapped in an envelope of [`Envelope::VERSION`].
    pub(super) fn seal(encoded: &str) -> String {
        format!(
            "{OPENING}{version}{SEPARATOR}{encoded}{CLOSING}",
            version = Self::VERSION
        )
    }

    /// The version the envelope names, as written.
    pub(super) fn version(&self) -> &'a str {
        self.version
    }

    /// What the envelope holds, or `None` when it is never closed.
    pub(super) fn encoded(&self) -> Option<&'a str> {
        self.rest.strip_suffix(CLOSING)
    }
}
