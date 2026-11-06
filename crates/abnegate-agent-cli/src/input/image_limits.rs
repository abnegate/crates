//! How much of a conversation's images one request may carry.

/// How much of a conversation's images one request to an agent may carry.
///
/// Images are kept newest first while they fit, and once one does not,
/// every older one is noted in its place rather than sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImageLimits {
    /// The base64 bytes one image may take.
    pub(crate) image_bytes: usize,
    /// How many images one request may carry.
    pub(crate) images: usize,
    /// The bytes one request may take, its text sized as the JSON it is sent
    /// as and its images as their base64 or their URL.
    pub(crate) request_bytes: usize,
}

impl ImageLimits {
    /// What a Claude run is held to: the Claude API's 10 MB of base64 an
    /// image, the 100 images a request may carry on its smallest context
    /// window, and its 128 MB a request, less room for what Claude Code adds.
    pub(crate) const CLAUDE: Self = Self {
        image_bytes: 10_000_000,
        images: 100,
        request_bytes: 127_000_000,
    };
}
