use crate::dataset::FrameKind;
use abnegate_vision::Rendered;

/// One crop of a person's photo, with the part of the person it frames and
/// its 8-bit subject mask when the model located the subject.
pub(crate) struct Frame {
    pub(crate) kind: FrameKind,
    pub(crate) rendered: Rendered,
    pub(crate) mask: Option<Vec<u8>>,
}
