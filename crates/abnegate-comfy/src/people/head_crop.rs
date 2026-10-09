use abnegate_vision::Region;

/// The head of a generated person, cut out for a face-refine pass, and where
/// the refined head goes back.
pub(crate) struct HeadCrop {
    /// The crop, encoded as PNG.
    pub(crate) png: Vec<u8>,
    /// Where the crop sits in the generated image.
    pub(crate) region: Region,
    /// Whether the crop is the whole image, so the refined image replaces it
    /// rather than being pasted into it.
    pub(crate) full_frame: bool,
}
