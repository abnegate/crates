//! How much of a conversation's images one request may carry.

/// How much of a conversation's images one request to an agent may carry.
///
/// Images are kept newest first while they fit, and once one does not,
/// every older one is noted in its place rather than sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// The base64 bytes one image may take.
    pub image_bytes: usize,
    /// How many images one request may carry.
    pub images: usize,
    /// The bytes one request may take, its text sized as the JSON it is sent
    /// as and its images as their base64 or their URL.
    pub request_bytes: usize,
}

impl Limits {
    /// What a Claude run is held to: the Claude API's 10 MB of base64 an
    /// image, the 100 images a request may carry on its smallest context
    /// window, and its 128 MB a request, less room for what Claude Code adds.
    pub const CLAUDE: Self = Self::new(10_000_000, 100, 127_000_000);

    /// Limits of `image_bytes` an image, `images` images and
    /// `request_bytes` a request.
    pub const fn new(image_bytes: usize, images: usize, request_bytes: usize) -> Self {
        Self {
            image_bytes,
            images,
            request_bytes,
        }
    }

    /// These limits with `image_bytes` an image.
    #[must_use]
    pub const fn with_image_bytes(mut self, image_bytes: usize) -> Self {
        self.image_bytes = image_bytes;
        self
    }

    /// These limits with `images` images a request.
    #[must_use]
    pub const fn with_images(mut self, images: usize) -> Self {
        self.images = images;
        self
    }

    /// These limits with `request_bytes` a request.
    #[must_use]
    pub const fn with_request_bytes(mut self, request_bytes: usize) -> Self {
        self.request_bytes = request_bytes;
        self
    }
}
