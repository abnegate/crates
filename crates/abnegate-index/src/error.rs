//! The result of every fallible operation in this crate.

/// The result of every fallible operation in this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong walking, embedding, or persisting chunks.
///
/// A variant may gain a field in a minor release, so a [`Store`](crate::Store)
/// or [`Embedder`](crate::Embedder) implemented outside this crate fails through
/// a constructor rather than a literal: [`Error::store`], [`Error::embed`], or
/// [`Error::cancel`]. A pattern outside this crate ends in `..`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The host store could not read or write chunks.
    #[error("Index store error: {message}")]
    #[non_exhaustive]
    Store {
        /// What the store reported.
        message: String,
    },
    /// The host embedder could not embed chunk text.
    #[error("Index embed error: {message}")]
    #[non_exhaustive]
    Embed {
        /// What the embedder reported.
        message: String,
    },
    /// The host asked the walk to stop.
    #[error("Index cancelled: {message}")]
    #[non_exhaustive]
    Cancel {
        /// What the host reported.
        message: String,
    },
}

impl Error {
    /// A store failure with `message`.
    pub fn store(message: impl Into<String>) -> Self {
        Self::Store {
            message: message.into(),
        }
    }

    /// An embedder failure with `message`.
    pub fn embed(message: impl Into<String>) -> Self {
        Self::Embed {
            message: message.into(),
        }
    }

    /// A cancellation with `message`.
    pub fn cancel(message: impl Into<String>) -> Self {
        Self::Cancel {
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_preserve_the_message() {
        assert_eq!(
            Error::store("disk full").to_string(),
            "Index store error: disk full"
        );
        assert_eq!(
            Error::embed("model missing").to_string(),
            "Index embed error: model missing"
        );
        assert_eq!(
            Error::cancel("timeout").to_string(),
            "Index cancelled: timeout"
        );
    }
}
