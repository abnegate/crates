//! An image type every agent here opens.

use crate::image::signature::Signature;

/// An image type, with the extension a file of it is saved under and the
/// signatures its bytes begin with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MediaType {
    pub(crate) name: &'static str,
    pub(crate) extension: &'static str,
    signatures: &'static [Signature],
}

impl MediaType {
    pub(crate) const fn new(
        name: &'static str,
        extension: &'static str,
        signatures: &'static [Signature],
    ) -> Self {
        Self {
            name,
            extension,
            signatures,
        }
    }

    /// Whether `bytes` are a file of this type.
    pub(crate) fn holds(&self, bytes: &[u8]) -> bool {
        self.signatures
            .iter()
            .any(|signature| signature.starts(bytes))
    }
}
