//! Decodes JPEG, PNG, and WebP images into an 8-bit RGB or RGBA raster.

use std::io::Cursor;

use image::codecs::jpeg::JpegDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::{ColorType, ImageDecoder};

/// The decoded-image ceiling, shared with the Go reference implementation.
/// Anything larger is rejected before a pixel buffer is allocated for it.
pub const MAX_PIXELS: u64 = 20_000_000;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported image format: unknown image format")]
    UnknownFormat,
    #[error("image dimensions are too large")]
    TooLarge,
    #[error("decode image: {0}")]
    Decode(#[from] image::ImageError),
}

/// The pixel layout of a decoded raster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Rgb,
    Rgba,
}

impl Layout {
    pub const fn channels(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Rgba => 4,
        }
    }
}

/// EXIF orientation, applied to the decoded pixels before analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    #[default]
    Normal,
    FlipHorizontal,
    Rotate180,
    FlipVertical,
    Transpose,
    Rotate90,
    Transverse,
    Rotate270,
}

impl Orientation {
    fn from_exif(value: u16) -> Self {
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

/// A decoded image and the orientation that still has to be applied to it.
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub layout: Layout,
    pub orientation: Orientation,
    pub pixels: Vec<u8>,
}

impl Raster {
    /// The dimensions the image has once its orientation is applied.
    pub const fn oriented_size(&self) -> (u32, u32) {
        if self.orientation.swaps_axes() {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

/// Decodes JPEG, PNG, or WebP data. EXIF orientation is recorded rather than
/// applied, so the caller can fold it into a later, much smaller resize.
pub fn decode(data: &[u8]) -> Result<Raster, Error> {
    match sniff(data).ok_or(Error::UnknownFormat)? {
        Format::Jpeg => {
            let decoder = JpegDecoder::new(Cursor::new(data))?;
            read(decoder, jpeg_orientation(data))
        }
        Format::Png => read(PngDecoder::new(Cursor::new(data))?, Orientation::Normal),
        Format::WebP => read(WebPDecoder::new(Cursor::new(data))?, Orientation::Normal),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Jpeg,
    Png,
    WebP,
}

fn sniff(data: &[u8]) -> Option<Format> {
    match data {
        [0xff, 0xd8, 0xff, ..] => Some(Format::Jpeg),
        [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, ..] => Some(Format::Png),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some(Format::WebP),
        _ => None,
    }
}

fn read<D: ImageDecoder>(decoder: D, orientation: Orientation) -> Result<Raster, Error> {
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(Error::TooLarge);
    }

    let color = decoder.color_type();
    let layout = match color {
        ColorType::Rgb8 | ColorType::L8 | ColorType::Rgb16 | ColorType::L16 => Layout::Rgb,
        _ => Layout::Rgba,
    };

    let mut raw = vec![0u8; decoder.total_bytes() as usize];
    decoder.read_image(&mut raw)?;

    let pixels = match color {
        ColorType::Rgb8 | ColorType::Rgba8 => raw,
        _ => widen(&raw, color),
    };

    Ok(Raster {
        width,
        height,
        layout,
        orientation,
        pixels,
    })
}

/// Expands grayscale and 16-bit samples to the 8-bit RGB or RGBA layout used by
/// the rest of the pipeline.
fn widen(raw: &[u8], color: ColorType) -> Vec<u8> {
    match color {
        ColorType::L8 => raw.iter().flat_map(|&luma| [luma, luma, luma]).collect(),
        ColorType::La8 => raw
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
            .collect(),
        ColorType::L16 => narrow(raw)
            .iter()
            .flat_map(|&luma| [luma, luma, luma])
            .collect(),
        ColorType::La16 => narrow(raw)
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
            .collect(),
        _ => narrow(raw),
    }
}

/// Reduces 16-bit samples to their high byte.
fn narrow(raw: &[u8]) -> Vec<u8> {
    raw.as_chunks::<2>()
        .0
        .iter()
        .map(|sample| (u16::from_ne_bytes(*sample) >> 8) as u8)
        .collect()
}

/// Reads the EXIF orientation tag out of a JPEG byte stream. JPEG is the only
/// format inspected, matching the auto-orientation the Go reference applies.
fn jpeg_orientation(data: &[u8]) -> Orientation {
    const MARKER_APP1: u8 = 0xe1;
    const MARKER_SOS: u8 = 0xda;
    const MARKER_EOI: u8 = 0xd9;

    let mut cursor = 2;
    while cursor + 4 <= data.len() {
        if data[cursor] != 0xff {
            return Orientation::Normal;
        }
        let marker = data[cursor + 1];
        if marker == MARKER_SOS || marker == MARKER_EOI {
            return Orientation::Normal;
        }
        let length = u16::from_be_bytes([data[cursor + 2], data[cursor + 3]]) as usize;
        if length < 2 {
            return Orientation::Normal;
        }
        let payload = cursor + 4;
        let end = payload + length - 2;
        if end > data.len() {
            return Orientation::Normal;
        }
        if marker == MARKER_APP1
            && let Some(exif) = data[payload..end].strip_prefix(b"Exif\0\0")
        {
            return exif_orientation(exif);
        }
        cursor = end;
    }
    Orientation::Normal
}

/// Parses the orientation tag out of a TIFF header and its first IFD.
fn exif_orientation(exif: &[u8]) -> Orientation {
    const TAG_ORIENTATION: u16 = 0x0112;
    const ENTRY_SIZE: usize = 12;

    if exif.len() < 8 {
        return Orientation::Normal;
    }
    let big_endian = match &exif[..2] {
        b"MM" => true,
        b"II" => false,
        _ => return Orientation::Normal,
    };
    let u16_at = |data: &[u8]| {
        let bytes = [data[0], data[1]];
        if big_endian {
            u16::from_be_bytes(bytes)
        } else {
            u16::from_le_bytes(bytes)
        }
    };
    let u32_at = |data: &[u8]| {
        let bytes = [data[0], data[1], data[2], data[3]];
        if big_endian {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        }
    };

    if u16_at(&exif[2..]) != 0x002a {
        return Orientation::Normal;
    }
    let directory = u32_at(&exif[4..]) as usize;
    if directory < 8 || directory + 2 > exif.len() {
        return Orientation::Normal;
    }
    let entries = u16_at(&exif[directory..]) as usize;
    for index in 0..entries {
        let entry = directory + 2 + index * ENTRY_SIZE;
        if entry + ENTRY_SIZE > exif.len() {
            break;
        }
        if u16_at(&exif[entry..]) == TAG_ORIENTATION {
            return Orientation::from_exif(u16_at(&exif[entry + 8..]));
        }
    }
    Orientation::Normal
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg_with_orientation(value: u16) -> Vec<u8> {
        let tiff: Vec<u8> = [
            &b"II\x2a\x00"[..],
            &[0x08, 0x00, 0x00, 0x00],
            &[0x01, 0x00],
            &[0x12, 0x01],
            &[0x03, 0x00],
            &[0x01, 0x00, 0x00, 0x00],
            &[value as u8, (value >> 8) as u8, 0x00, 0x00],
            &[0x00, 0x00, 0x00, 0x00],
        ]
        .concat();
        let payload = [&b"Exif\0\0"[..], &tiff].concat();
        let length = (payload.len() + 2) as u16;
        [
            &[0xff, 0xd8, 0xff, 0xe1][..],
            &length.to_be_bytes(),
            &payload,
        ]
        .concat()
    }

    #[test]
    fn reads_jpeg_orientation() {
        assert_eq!(
            jpeg_orientation(&jpeg_with_orientation(6)),
            Orientation::Rotate90
        );
        assert_eq!(
            jpeg_orientation(&jpeg_with_orientation(8)),
            Orientation::Rotate270
        );
        assert_eq!(
            jpeg_orientation(&jpeg_with_orientation(1)),
            Orientation::Normal
        );
        assert_eq!(
            jpeg_orientation(&jpeg_with_orientation(99)),
            Orientation::Normal
        );
    }

    #[test]
    fn rejects_unknown_format() {
        assert!(matches!(decode(b"not an image"), Err(Error::UnknownFormat)));
    }

    #[test]
    fn sniffs_supported_formats() {
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some(Format::Jpeg));
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n\0"), Some(Format::Png));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(Format::WebP));
        assert_eq!(sniff(b"GIF89a..."), None);
    }
}
