use crate::video::Frame;
use serde::Serialize;

/// What one clip yielded.
#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct Clip {
    pub frames: Vec<Frame>,
    /// Frames pulled out of the video before selection.
    pub sampled: usize,
    /// Rate those frames were pulled at, which drops below the requested rate
    /// only when the clip is long enough to hit the sampling ceiling.
    pub sampled_fps: f64,
}
