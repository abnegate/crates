use std::io;

use crate::error::Error;

/// Fill `bytes` from the operating system's random source.
pub(crate) fn fill(bytes: &mut [u8]) -> Result<(), Error> {
    getrandom::fill(bytes).map_err(|error| Error::Entropy {
        source: io::Error::from(error),
    })
}
