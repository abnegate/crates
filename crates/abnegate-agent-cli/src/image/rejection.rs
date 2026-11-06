//! Why an attached image cannot be shown to an agent.

use thiserror::Error;

/// Why an image a message carries cannot be shown to an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum Rejection {
    /// It is not a PNG, JPEG, GIF or WebP image.
    #[error("it is not a PNG, JPEG, GIF or WebP image")]
    Unsupported,
    /// Its data could not be decoded, or decodes to no image of a type
    /// agents open.
    #[error("its data could not be decoded as the image it says it is")]
    Malformed,
    /// It is neither a base64 data URL nor an `http` or `https` link, so the
    /// agent has no way to read it.
    #[error("it is neither inline data nor a web address the agent can open")]
    Unreachable,
    /// It is a link, and the agent opens images only from files.
    #[error("it is a link, and codex opens images only from files")]
    Linked,
    /// It is larger than one image in a request may be.
    #[error("it is larger than one image may be")]
    Oversized,
    /// The conversation already carries as many images as one request may.
    #[error("the conversation already carries as many images as a turn may hold")]
    Excess,
    /// The conversation's images together pass what one request may carry.
    #[error("the conversation's images together pass what one turn may carry")]
    Crowded,
}
