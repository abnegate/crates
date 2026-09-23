//! The media types ComfyUI generates and this crate collects.
//!
//! One table, so a filename, a stored artifact, and a chat attachment can never
//! disagree about what a `.flac` or a `.opus` is.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lane {
    Image,
    Video,
    Audio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct MediaType {
    lane: Lane,
    /// The media type emitted and stored for this format.
    pub mime: &'static str,
    /// The canonical file suffix for [`MediaType::mime`].
    pub extension: &'static str,
    /// Suffixes and media types that also denote this format on input but that
    /// are never emitted. Suffixes and media types never collide lexically, so
    /// one list serves both lookups.
    aliases: &'static [&'static str],
}

impl MediaType {
    pub const PNG: Self = Self {
        lane: Lane::Image,
        mime: "image/png",
        extension: "png",
        aliases: &[],
    };
    pub const JPEG: Self = Self {
        lane: Lane::Image,
        mime: "image/jpeg",
        extension: "jpg",
        aliases: &["jpeg"],
    };
    pub const WEBP: Self = Self {
        lane: Lane::Image,
        mime: "image/webp",
        extension: "webp",
        aliases: &[],
    };
    pub const GIF: Self = Self {
        lane: Lane::Image,
        mime: "image/gif",
        extension: "gif",
        aliases: &[],
    };
    pub const AVIF: Self = Self {
        lane: Lane::Image,
        mime: "image/avif",
        extension: "avif",
        aliases: &[],
    };
    pub const WEBM: Self = Self {
        lane: Lane::Video,
        mime: "video/webm",
        extension: "webm",
        aliases: &[],
    };
    pub const MP4: Self = Self {
        lane: Lane::Video,
        mime: "video/mp4",
        extension: "mp4",
        aliases: &[],
    };
    pub const FLAC: Self = Self {
        lane: Lane::Audio,
        mime: "audio/flac",
        extension: "flac",
        aliases: &[],
    };
    pub const MP3: Self = Self {
        lane: Lane::Audio,
        mime: "audio/mpeg",
        extension: "mp3",
        aliases: &[],
    };
    /// RFC 7845 Ogg-encapsulated Opus. RFC 7587's `audio/opus` names a bare RTP
    /// payload, which no browser will play from an `<audio>` element, so it is
    /// accepted on input and normalised away on output.
    pub const OPUS: Self = Self {
        lane: Lane::Audio,
        mime: "audio/ogg",
        extension: "opus",
        aliases: &["audio/opus", "ogg"],
    };
    pub const WAV: Self = Self {
        lane: Lane::Audio,
        mime: "audio/wav",
        extension: "wav",
        aliases: &[],
    };

    const ALL: &'static [Self] = &[
        Self::PNG,
        Self::JPEG,
        Self::WEBP,
        Self::GIF,
        Self::AVIF,
        Self::WEBM,
        Self::MP4,
        Self::FLAC,
        Self::MP3,
        Self::OPUS,
        Self::WAV,
    ];

    /// Look a format up by the suffix of `name`, which may be a bare filename,
    /// a path, or a URL. Matching is case-insensitive.
    pub fn for_filename(name: &str) -> Option<Self> {
        let (_, extension) = name.rsplit_once('.')?;
        Self::for_extension(extension)
    }

    pub fn for_extension(extension: &str) -> Option<Self> {
        let extension = extension.to_ascii_lowercase();
        Self::ALL
            .iter()
            .find(|media| media.extension == extension || media.aliases.contains(&&*extension))
            .copied()
    }

    pub fn for_mime(mime: &str) -> Option<Self> {
        let mime = mime.trim().to_ascii_lowercase();
        Self::ALL
            .iter()
            .find(|media| media.mime == mime || media.aliases.contains(&&*mime))
            .copied()
    }

    pub fn is_image(&self) -> bool {
        self.lane == Lane::Image
    }

    pub fn is_video(&self) -> bool {
        self.lane == Lane::Video
    }

    pub fn is_audio(&self) -> bool {
        self.lane == Lane::Audio
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_files_are_ogg_encapsulated() {
        let opus = MediaType::for_filename("clip.opus").expect("a known suffix");
        assert_eq!(
            opus.mime, "audio/ogg",
            "RFC 7587 audio/opus is an RTP payload; browsers refuse to play it"
        );
        assert_eq!(MediaType::for_mime("audio/ogg"), Some(MediaType::OPUS));
    }

    #[test]
    fn the_rtp_opus_media_type_is_accepted_but_never_emitted() {
        let opus = MediaType::for_mime("audio/opus").expect("a tolerated alias");
        assert_eq!(opus.extension, "opus");
        assert_eq!(opus.mime, "audio/ogg");
    }

    #[test]
    fn suffix_lookup_ignores_case() {
        for name in ["clip.FLAC", "clip.Flac", "/api/artifacts/w/c/m/x.FLAC"] {
            assert_eq!(
                MediaType::for_filename(name),
                Some(MediaType::FLAC),
                "{name} should be recognised as FLAC"
            );
        }
        assert_eq!(MediaType::for_mime("AUDIO/FLAC"), Some(MediaType::FLAC));
    }

    #[test]
    fn jpeg_answers_to_both_suffixes_but_stores_one() {
        assert_eq!(MediaType::for_filename("photo.jpeg"), Some(MediaType::JPEG));
        assert_eq!(MediaType::for_filename("photo.jpg"), Some(MediaType::JPEG));
        assert_eq!(MediaType::JPEG.extension, "jpg");
    }

    #[test]
    fn matroska_is_not_a_format_that_can_be_stored() {
        assert_eq!(
            MediaType::for_filename("clip.mkv"),
            None,
            "the artifact store rejects .mkv, so the pipeline must not collect one"
        );
        assert_eq!(MediaType::for_mime("video/x-matroska"), None);
    }

    #[test]
    fn unknown_and_suffixless_names_have_no_type() {
        assert_eq!(MediaType::for_filename("clip"), None);
        assert_eq!(MediaType::for_filename("clip.txt"), None);
        assert_eq!(MediaType::for_mime("text/html"), None);
        assert_eq!(MediaType::for_mime(""), None);
    }

    #[test]
    fn every_format_belongs_to_exactly_one_lane() {
        for media in MediaType::ALL {
            let lanes = [media.is_image(), media.is_video(), media.is_audio()];
            assert_eq!(
                lanes.iter().filter(|belongs| **belongs).count(),
                1,
                "{} belongs to {lanes:?}",
                media.mime
            );
            assert_eq!(
                media.is_image(),
                media.mime.starts_with("image/"),
                "{} is filed under the wrong lane",
                media.mime
            );
            assert_eq!(
                media.is_audio(),
                media.mime.starts_with("audio/"),
                "{} is filed under the wrong lane",
                media.mime
            );
        }
    }

    #[test]
    fn every_canonical_suffix_and_media_type_is_unique() {
        for (index, media) in MediaType::ALL.iter().enumerate() {
            for other in &MediaType::ALL[index + 1..] {
                assert_ne!(
                    media.extension, other.extension,
                    "{} and {} claim the same suffix",
                    media.mime, other.mime
                );
                assert_ne!(media.mime, other.mime, "{} is listed twice", media.mime);
            }
            for alias in media.aliases {
                assert_ne!(
                    *alias, media.extension,
                    "{} lists its own suffix as an alias",
                    media.mime
                );
            }
        }
    }
    #[test]
    fn ogg_files_resolve_to_the_opus_audio_type() {
        let media = MediaType::for_filename("clip.ogg").expect("ogg is an audio container");
        assert_eq!(media.mime, "audio/ogg");
        assert_eq!(media.extension, "opus");
        assert_eq!(MediaType::for_extension("OGG"), Some(MediaType::OPUS));
    }
}
