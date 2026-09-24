/// Frames a clip contributes unless [`Options::with_limit`] says otherwise.
const DEFAULT_LIMIT: usize = 48;

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
    /// Keeps `fps` frames a second, each rendered as a `resolution`-pixel
    /// square, mirrors alternate frames, and keeps at most 48 in all.
    pub const fn new(fps: u32, resolution: u32) -> Self {
        Self {
            fps,
            resolution,
            mirror: true,
            limit: DEFAULT_LIMIT,
        }
    }

    /// The same options, mirroring alternate frames only when `mirror` is set.
    /// Worth turning off for a subject carrying text or anything else a mirror
    /// would render backwards.
    pub const fn with_mirror(mut self, mirror: bool) -> Self {
        self.mirror = mirror;
        self
    }

    /// The same options, keeping at most `limit` frames in all.
    pub const fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_mirror_and_keep_forty_eight_frames_unless_told_otherwise() {
        let options = Options::new(4, 512);
        assert_eq!(
            (
                options.fps,
                options.resolution,
                options.mirror,
                options.limit
            ),
            (4, 512, true, 48)
        );

        let options = options.with_mirror(false).with_limit(12);
        assert_eq!(
            (
                options.fps,
                options.resolution,
                options.mirror,
                options.limit
            ),
            (4, 512, false, 12)
        );
    }
}
