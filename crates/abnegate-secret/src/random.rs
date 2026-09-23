use std::io;

use crate::error::SecretError;

/// Fill `bytes` from the operating system's random source.
pub(crate) fn fill(bytes: &mut [u8]) -> Result<(), SecretError> {
    getrandom::fill(bytes).map_err(|error| SecretError::Entropy {
        source: io::Error::from(error),
    })
}
