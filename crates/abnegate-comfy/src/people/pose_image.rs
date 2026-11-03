/// The OpenPose image a person's generation is posed with, ready to upload.
pub(crate) struct PoseImage {
    /// The name it is uploaded under.
    pub(crate) name: String,
    /// The encoded image.
    pub(crate) bytes: Vec<u8>,
}
