/// How a clip is turned into training images.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Options {
    /// Frames kept per second of video.
    pub fps: u32,
    /// Side of the square crop every frame is rendered at.
    pub resolution: u32,
    /// Mirror alternate frames within each second.
    pub mirror: bool,
    /// Frames kept in total. The packaged trainer caps its step count, so past
    /// this each extra frame is seen fewer times without adding variety the
    /// selection has not already taken.
    pub limit: usize,
}

impl Options {
    /// Keeps `fps` frames a second, at most `limit` in all, each rendered as a
    /// `resolution`-pixel square and mirrored in alternation when `mirror` is
    /// set.
    pub const fn new(fps: u32, resolution: u32, mirror: bool, limit: usize) -> Self {
        Self {
            fps,
            resolution,
            mirror,
            limit,
        }
    }
}
