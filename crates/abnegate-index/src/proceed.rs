//! A [`Cancel`](crate::Cancel) that never stops the walk.

use crate::cancel::Cancel;
use crate::error::Result;

/// A [`Cancel`](crate::Cancel) that never stops the walk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Proceed;

impl Cancel for Proceed {
    fn check(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proceed_always_allows() {
        assert!(Proceed.check().is_ok());
    }
}
