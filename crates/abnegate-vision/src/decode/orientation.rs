//! The EXIF orientation still to be applied to a raster.

/// EXIF orientation, applied to the decoded pixels before analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Orientation {
    /// EXIF 1, and any value outside 1 to 8: stored upright.
    #[default]
    Normal,
    /// EXIF 2: mirrored left to right.
    FlipHorizontal,
    /// EXIF 3: turned half a revolution.
    Rotate180,
    /// EXIF 4: mirrored top to bottom.
    FlipVertical,
    /// EXIF 5: mirrored across the top-left to bottom-right diagonal.
    Transpose,
    /// EXIF 6: turned a quarter revolution clockwise to display.
    Rotate90,
    /// EXIF 7: mirrored across the top-right to bottom-left diagonal.
    Transverse,
    /// EXIF 8: turned a quarter revolution anticlockwise to display.
    Rotate270,
}

impl Orientation {
    pub(crate) fn from_exif(value: u16) -> Self {
        match value {
            2 => Self::FlipHorizontal,
            3 => Self::Rotate180,
            4 => Self::FlipVertical,
            5 => Self::Transpose,
            6 => Self::Rotate90,
            7 => Self::Transverse,
            8 => Self::Rotate270,
            _ => Self::Normal,
        }
    }

    /// Reports whether the transform exchanges the width and height axes.
    pub const fn swaps_axes(self) -> bool {
        matches!(
            self,
            Self::Transpose | Self::Rotate90 | Self::Transverse | Self::Rotate270
        )
    }
}
