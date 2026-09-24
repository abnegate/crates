//! Decodes JPEG, PNG, and WebP images into an 8-bit RGB or RGBA raster.

use std::io::Cursor;

use image::codecs::jpeg::JpegDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::{ColorType, ImageDecoder, Limits};

use crate::decode::format::Format;

mod error;
mod format;
mod layout;
mod orientation;
mod raster;

pub use crate::decode::error::DecodeError;
pub use crate::decode::layout::Layout;
pub use crate::decode::orientation::Orientation;
pub use crate::decode::raster::Raster;

/// The decoded-image ceiling, shared with the Go reference implementation.
/// Anything larger is rejected before a pixel buffer is allocated for it.
pub const MAXIMUM_PIXELS: u64 = 20_000_000;

/// The widest pixel a supported format decodes to: four 16-bit channels.
const MAXIMUM_BYTES_PER_PIXEL: u64 = 8;

/// What a decoder may allocate for one image, its own pixels included.
const MAXIMUM_ALLOCATION: u64 = MAXIMUM_PIXELS * MAXIMUM_BYTES_PER_PIXEL;

/// Decodes JPEG, PNG, or WebP data. EXIF orientation is recorded rather than
/// applied, so the caller can fold it into a later, much smaller resize.
///
/// Each decoder is held to an allocation budget sized for the largest image
/// [`MAXIMUM_PIXELS`] admits, so a small file cannot talk one into allocating
/// far more than the image it describes.
pub fn decode(data: &[u8]) -> Result<Raster, DecodeError> {
    decode_within(data, MAXIMUM_ALLOCATION)
}

fn decode_within(data: &[u8], budget: u64) -> Result<Raster, DecodeError> {
    let limits = limits(budget);
    match Format::sniff(data).ok_or(DecodeError::UnknownFormat)? {
        Format::Jpeg => {
            let mut decoder = JpegDecoder::new(Cursor::new(data))?;
            decoder.set_limits(limits)?;
            read(decoder, jpeg_orientation(data))
        }
        Format::Png => read(
            PngDecoder::with_limits(Cursor::new(data), limits)?,
            Orientation::Normal,
        ),
        Format::WebP => {
            let mut decoder = WebPDecoder::new(Cursor::new(data))?;
            decoder.set_limits(limits)?;
            read(decoder, Orientation::Normal)
        }
    }
}

fn limits(budget: u64) -> Limits {
    let mut limits = Limits::no_limits();
    limits.max_alloc = Some(budget);
    limits
}

fn read<D: ImageDecoder>(decoder: D, orientation: Orientation) -> Result<Raster, DecodeError> {
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAXIMUM_PIXELS {
        return Err(DecodeError::TooLarge);
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

    Ok(Raster::new(width, height, layout, pixels).with_orientation(orientation))
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
        assert!(matches!(
            decode(b"not an image"),
            Err(DecodeError::UnknownFormat)
        ));
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// A 2x2 PNG carrying a `tEXt` chunk of `text_bytes`, which the decoder
    /// has to buffer whole before it reaches a pixel.
    fn png_with_text(text_bytes: usize) -> Vec<u8> {
        use image::ImageEncoder;
        use image::codecs::png::PngEncoder;

        let mut encoded = Vec::new();
        PngEncoder::new(&mut encoded)
            .write_image(&[0u8; 12], 2, 2, image::ExtendedColorType::Rgb8)
            .expect("encode");

        let data = [&b"Comment\0"[..], &vec![b'x'; text_bytes]].concat();
        let body = [&b"tEXt"[..], &data].concat();
        let chunk = [
            &u32::try_from(data.len()).expect("length").to_be_bytes()[..],
            &body,
            &crc32(&body).to_be_bytes(),
        ]
        .concat();

        let after_header = 8 + 4 + 4 + 13 + 4;
        [&encoded[..after_header], &chunk, &encoded[after_header..]].concat()
    }

    #[test]
    fn a_chunk_beyond_the_allocation_budget_is_refused() {
        let image = png_with_text(64 * 1024);
        assert!(decode(&image).is_ok(), "the full budget admits it");

        let error = decode_within(&image, 1024)
            .err()
            .expect("the decoder has to stop at the budget");
        assert!(
            matches!(error, DecodeError::Decode(image::ImageError::Limits(_))),
            "{error:?}"
        );
    }

    #[test]
    fn the_budget_covers_the_widest_image_admitted() {
        assert_eq!(
            limits(MAXIMUM_ALLOCATION).max_alloc,
            Some(MAXIMUM_PIXELS * 8)
        );
    }
}
