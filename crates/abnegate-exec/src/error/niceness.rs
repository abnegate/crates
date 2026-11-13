use thiserror::Error;

/// Errors from naming a [`Niceness`](crate::Niceness).
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum NicenessError {
    /// The value lay past 19, the lowest priority a process can take
    #[error("Not a nice value from 0 to 19: {0}")]
    OutOfRange(u8),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_out_of_range_value_names_itself() {
        assert_eq!(
            NicenessError::OutOfRange(20).to_string(),
            "Not a nice value from 0 to 19: 20"
        );
    }
}
