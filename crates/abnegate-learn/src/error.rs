//! The result of every fallible operation in this crate.

/// The result of every fallible operation in this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong recording, embedding, or persisting trials.
///
/// A variant may gain a field in a minor release, so an [`Archive`](crate::Archive)
/// or [`Embedder`](crate::Embedder) implemented outside this crate fails through
/// a constructor rather than a literal: [`Error::archive`] or [`Error::embed`].
/// A pattern outside this crate ends in `..`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The host archive could not load or save a trial.
    #[error("Learn archive error: {message}")]
    #[non_exhaustive]
    Archive {
        /// What the archive reported.
        message: String,
    },
    /// The host embedder could not embed trial text.
    #[error("Learn embed error: {message}")]
    #[non_exhaustive]
    Embed {
        /// What the embedder reported.
        message: String,
    },
}

impl Error {
    /// An archive failure with `message`.
    pub fn archive(message: impl Into<String>) -> Self {
        Self::Archive {
            message: message.into(),
        }
    }

    /// An embedder failure with `message`.
    pub fn embed(message: impl Into<String>) -> Self {
        Self::Embed {
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
            Error::archive("disk full").to_string(),
            "Learn archive error: disk full"
        );
        assert_eq!(
            Error::embed("model missing").to_string(),
            "Learn embed error: model missing"
        );
    }
}
