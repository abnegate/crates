use crate::lora::TrainError;
use base64::Engine;
use serde::Deserialize;
use std::borrow::Cow;
use std::fmt;

/// One image of a training set, as base64 from a JSON body or as raw bytes
/// from a multipart upload. Raw bytes that are not empty win over the base64,
/// so a large set never has to round-trip through JSON.
#[derive(Deserialize)]
#[non_exhaustive]
pub struct TrainImage {
    /// Name the image was uploaded under.
    pub filename: String,
    /// Its caption, or for an edit base the instruction that turns the
    /// reference into it. Blank asks the vision model for one.
    pub caption: String,
    /// The encoded image, in base64.
    #[serde(default)]
    pub bytes_base64: String,
    /// The encoded image, as uploaded.
    #[serde(skip)]
    pub bytes: Option<Vec<u8>>,
    /// The reference an edit adapter learns to change into this image, in
    /// base64. Only edit bases take one.
    #[serde(default)]
    pub before_base64: Option<String>,
    /// The reference, as uploaded.
    #[serde(skip)]
    pub before: Option<Vec<u8>>,
    /// Images sharing a group are the same shot and are captioned together.
    /// Frames pulled from a clip arrive grouped; separate photos do not.
    #[serde(default)]
    pub group: Option<usize>,
}

impl TrainImage {
    /// An image named `filename`, captioned `caption`, from its base64
    /// `bytes_base64`, with no reference and no shot of its own.
    pub fn new(
        filename: impl Into<String>,
        caption: impl Into<String>,
        bytes_base64: impl Into<String>,
    ) -> Self {
        Self {
            filename: filename.into(),
            caption: caption.into(),
            bytes_base64: bytes_base64.into(),
            bytes: None,
            before_base64: None,
            before: None,
            group: None,
        }
    }

    /// An image named `filename`, captioned `caption`, from its encoded
    /// `bytes` as uploaded, with no reference and no shot of its own.
    pub fn from_bytes(
        filename: impl Into<String>,
        caption: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            bytes: Some(bytes.into()),
            ..Self::new(filename, caption, String::new())
        }
    }

    /// Sets the reference, in base64, that an edit adapter learns to change
    /// into this image.
    pub fn with_before_base64(mut self, before_base64: impl Into<String>) -> Self {
        self.before_base64 = Some(before_base64.into());
        self
    }

    /// Sets the reference, as uploaded, that an edit adapter learns to change
    /// into this image.
    pub fn with_before(mut self, before: impl Into<Vec<u8>>) -> Self {
        self.before = Some(before.into());
        self
    }

    /// Places the image in the shot `group`, so it is captioned with the rest
    /// of that shot.
    pub fn with_group(mut self, group: usize) -> Self {
        self.group = Some(group);
        self
    }

    /// The encoded image: the uploaded bytes, borrowed, unless they are empty,
    /// or else the base64 decoded. Fails with [`TrainError::Invalid`] when
    /// both are empty or the base64 is not base64.
    pub fn encoded(&self) -> Result<Cow<'_, [u8]>, TrainError> {
        encoded(self.bytes.as_deref(), &self.bytes_base64)
    }

    /// The encoded reference, taken the way [`TrainImage::encoded`] takes the
    /// image, or `None` when [`TrainImage::has_reference`] finds none.
    pub fn encoded_reference(&self) -> Result<Option<Cow<'_, [u8]>>, TrainError> {
        if !self.has_reference() {
            return Ok(None);
        }
        encoded(
            self.before.as_deref(),
            self.before_base64.as_deref().unwrap_or_default(),
        )
        .map(Some)
    }

    /// Whether the image carries a reference that is not blank.
    pub fn has_reference(&self) -> bool {
        self.before
            .as_ref()
            .is_some_and(|before| !before.is_empty())
            || self
                .before_base64
                .as_ref()
                .is_some_and(|before| !before.trim().is_empty())
    }

    /// Moves the encoded image out, leaving neither the bytes nor the base64
    /// behind, so a large set is held once rather than twice.
    pub(crate) fn take_encoded(&mut self) -> Result<Vec<u8>, TrainError> {
        let base64 = std::mem::take(&mut self.bytes_base64);
        match self.bytes.take().filter(|bytes| !bytes.is_empty()) {
            Some(bytes) => Ok(bytes),
            None => decode_base64(&base64),
        }
    }
}

impl fmt::Debug for TrainImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TrainImage")
            .field("filename", &self.filename)
            .field("caption", &self.caption)
            .field("bytes_base64", &self.bytes_base64.len())
            .field("bytes", &self.bytes.as_ref().map(Vec::len))
            .field(
                "before_base64",
                &self.before_base64.as_ref().map(String::len),
            )
            .field("before", &self.before.as_ref().map(Vec::len))
            .field("group", &self.group)
            .finish()
    }
}

fn encoded<'a>(bytes: Option<&'a [u8]>, base64: &str) -> Result<Cow<'a, [u8]>, TrainError> {
    match bytes.filter(|bytes| !bytes.is_empty()) {
        Some(bytes) => Ok(Cow::Borrowed(bytes)),
        None => decode_base64(base64).map(Cow::Owned),
    }
}

fn decode_base64(base64: &str) -> Result<Vec<u8>, TrainError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64.trim())
        .map_err(|_| TrainError::Invalid("image is not valid base64"))?;
    if bytes.is_empty() {
        return Err(TrainError::Invalid("image is empty"));
    }
    Ok(bytes)
}
