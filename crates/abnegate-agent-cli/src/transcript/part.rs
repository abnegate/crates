//! A piece of a rendered conversation.

/// A piece of a rendered conversation, in the order the agent reads it: the
/// text [`render`](crate::transcript::render) writes, with each message's
/// images after the message they were attached to.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Part {
    /// Messages rendered under their roles' labels, back to back.
    Text(String),
    /// An image attached to the message rendered before it.
    #[non_exhaustive]
    Image {
        /// The URL the image was attached by, as the message carries it.
        url: String,
        /// Whether it is one of the images the last user message carries,
        /// the ones the turn answering it was sent with.
        current: bool,
    },
}

impl Part {
    /// Rendered text.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    /// The image at `url`, one the turn was sent with when `current`.
    pub fn image(url: impl Into<String>, current: bool) -> Self {
        Self::Image {
            url: url.into(),
            current,
        }
    }
}
