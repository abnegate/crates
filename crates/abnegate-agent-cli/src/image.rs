//! An image a message carries, read from the URL it was attached by.

mod image_error;
mod media_type;
mod signature;

use std::borrow::Cow;

use base64::Engine as _;
use base64::alphabet::STANDARD;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::GeneralPurpose;
use base64::engine::general_purpose::GeneralPurposeConfig;

use crate::image::media_type::MediaType;
use crate::image::signature::Signature;

pub use crate::image::image_error::ImageError;

const DATA_SCHEME: &str = "data:";
const BASE64_SUFFIX: &str = ";base64";
const LINK_SCHEMES: &[&str] = &["https://", "http://"];
const BASE64_QUANTUM: usize = 4;

const MEDIA_TYPES: &[MediaType] = &[
    MediaType::new(
        "image/png",
        "png",
        &[Signature::Prefix(b"\x89PNG\r\n\x1a\n")],
    ),
    MediaType::new("image/jpeg", "jpg", &[Signature::Prefix(b"\xff\xd8\xff")]),
    MediaType::new(
        "image/gif",
        "gif",
        &[Signature::Prefix(b"GIF87a"), Signature::Prefix(b"GIF89a")],
    ),
    MediaType::new("image/webp", "webp", &[Signature::Riff(b"WEBP")]),
];

const LENIENT: GeneralPurpose = GeneralPurpose::new(
    &STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// An image a message carries, as an agent can be shown it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Image<'a> {
    /// The decoded bytes of a base64 data URL, as the type they are.
    #[non_exhaustive]
    Inline {
        /// The lowercase media type the bytes are, such as `image/png`,
        /// whatever type the URL labelled them with.
        media_type: &'static str,
        /// The extension a file of that type is saved under, such as `png`.
        extension: &'static str,
        /// The image's bytes.
        bytes: Vec<u8>,
    },
    /// An `http` or `https` address, which the agent fetches itself.
    Linked(&'a str),
}

impl<'a> Image<'a> {
    /// Read the image at `url`: a base64 data URL of a PNG, JPEG, GIF or
    /// WebP image, or a web address.
    ///
    /// A data URL's media type is read in any case and its base64 wrapped or
    /// unpadded. Its bytes must begin as an image of one of those types does,
    /// and are read as the type they are rather than the one they were
    /// labelled with.
    ///
    /// # Errors
    ///
    /// [`ImageError::Unsupported`] for a data URL of any other type,
    /// [`ImageError::Malformed`] for one that does not decode to such an
    /// image, and [`ImageError::Unreachable`] for any other address.
    pub fn parse(url: &'a str) -> Result<Self, ImageError> {
        if let Some(rest) = url.strip_prefix(DATA_SCHEME) {
            let (header, data) = rest.split_once(',').ok_or(ImageError::Malformed)?;
            let declared = header
                .strip_suffix(BASE64_SUFFIX)
                .ok_or(ImageError::Malformed)?;
            if !MEDIA_TYPES
                .iter()
                .any(|known| known.name.eq_ignore_ascii_case(declared))
            {
                return Err(ImageError::Unsupported);
            }
            let bytes = LENIENT
                .decode(compact(data).as_bytes())
                .map_err(|_| ImageError::Malformed)?;
            let actual = MEDIA_TYPES
                .iter()
                .find(|known| known.holds(&bytes))
                .ok_or(ImageError::Malformed)?;
            return Ok(Self::Inline {
                media_type: actual.name,
                extension: actual.extension,
                bytes,
            });
        }
        if LINK_SCHEMES.iter().any(|scheme| url.starts_with(scheme)) {
            return Ok(Self::Linked(url));
        }
        Err(ImageError::Unreachable)
    }
}

/// The bytes the base64 of the image a data URL carries takes once padded,
/// counted without decoding it, or `None` for any other URL.
pub(crate) fn encoded_length(url: &str) -> Option<usize> {
    let (_, data) = url.strip_prefix(DATA_SCHEME)?.split_once(',')?;
    let length = data
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .count();
    Some(length.div_ceil(BASE64_QUANTUM) * BASE64_QUANTUM)
}

fn compact(data: &str) -> Cow<'_, str> {
    if data.bytes().any(|byte| byte.is_ascii_whitespace()) {
        Cow::Owned(
            data.chars()
                .filter(|character| !character.is_ascii_whitespace())
                .collect(),
        )
    } else {
        Cow::Borrowed(data)
    }
}

#[cfg(test)]
mod tests {
    use super::Image;
    use super::ImageError;
    use super::encoded_length;

    const PNG_BYTES: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

    fn inline(media_type: &'static str, extension: &'static str) -> Image<'static> {
        Image::Inline {
            media_type,
            extension,
            bytes: PNG_BYTES.to_vec(),
        }
    }

    #[test]
    fn a_data_urls_encoded_length_is_its_padded_base64() {
        for url in [
            "data:image/png;base64,iVBORw0KGgo=",
            "data:image/png;base64,iVBORw0KGgo",
            "data:image/png;base64,iVBORw0K\nGgo=",
        ] {
            assert_eq!(encoded_length(url), Some(12), "{url}");
        }
        assert_eq!(encoded_length("https://example.test/shot.png"), None);
        assert_eq!(encoded_length("data:image/png;base64"), None);
    }

    #[test]
    fn a_base64_data_url_is_an_inline_image() {
        assert_eq!(
            Image::parse("data:image/png;base64,iVBORw0KGgo="),
            Ok(inline("image/png", "png"))
        );
    }

    #[test]
    fn a_media_type_in_any_case_is_read_as_the_lowercase_one_agents_take() {
        assert_eq!(
            Image::parse("data:IMAGE/PNG;base64,iVBORw0KGgo="),
            Ok(inline("image/png", "png"))
        );
    }

    #[test]
    fn wrapped_or_unpadded_base64_still_decodes() {
        for url in [
            "data:image/png;base64,iVBORw0K\nGgo=",
            "data:image/png;base64,iVBORw0KGgo",
        ] {
            assert_eq!(Image::parse(url), Ok(inline("image/png", "png")), "{url}");
        }
    }

    #[test]
    fn an_image_is_read_as_the_type_its_bytes_are_whatever_it_was_labelled() {
        assert_eq!(
            Image::parse("data:image/png;base64,/9j/4AAQ"),
            Ok(Image::Inline {
                media_type: "image/jpeg",
                extension: "jpg",
                bytes: vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10],
            })
        );
        assert_eq!(
            Image::parse("data:image/png;base64,UklGRiQAAABXRUJQVlA4"),
            Ok(Image::Inline {
                media_type: "image/webp",
                extension: "webp",
                bytes: b"RIFF$\0\0\0WEBPVP8".to_vec(),
            })
        );
    }

    #[test]
    fn a_gif_of_either_version_is_read_as_one() {
        for (data, version) in [("R0lGODdh", "87a"), ("R0lGODlh", "89a")] {
            assert_eq!(
                Image::parse(&format!("data:image/gif;base64,{data}")),
                Ok(Image::Inline {
                    media_type: "image/gif",
                    extension: "gif",
                    bytes: format!("GIF{version}").into_bytes(),
                }),
                "{version}"
            );
        }
    }

    #[test]
    fn a_public_address_is_a_linked_image() {
        for url in [
            "https://example.test/shot.png",
            "http://example.test/shot.png",
        ] {
            assert_eq!(Image::parse(url), Ok(Image::Linked(url)));
        }
    }

    #[test]
    fn an_image_no_agent_can_be_shown_says_why() {
        for (url, error) in [
            (
                "/api/artifacts/workspace/chat/message/generated.png",
                ImageError::Unreachable,
            ),
            ("ftp://example.test/shot.png", ImageError::Unreachable),
            (
                "data:image/svg+xml;base64,PHN2Zz4=",
                ImageError::Unsupported,
            ),
            ("data:text/plain;base64,aGk=", ImageError::Unsupported),
            ("data:image/png,not-base64", ImageError::Malformed),
            ("data:image/png;base64,", ImageError::Malformed),
            ("data:image/png;base64,!!!!", ImageError::Malformed),
            ("data:image/png;base64,aGVsbG8=", ImageError::Malformed),
        ] {
            assert_eq!(Image::parse(url), Err(error), "{url}");
        }
    }

    #[test]
    fn an_image_error_names_no_host_of_its_own() {
        assert_eq!(
            ImageError::Unreachable.to_string(),
            "it is neither inline data nor a web address the agent can open"
        );
    }
}
