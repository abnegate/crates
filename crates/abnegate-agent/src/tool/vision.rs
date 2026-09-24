const NON_VISION_EXTENSIONS: &[&str] = &[
    ".flac", ".mp3", ".opus", ".wav", ".ogg", ".m4a", ".aac", ".webm", ".mp4", ".mkv", ".mov",
];

/// Whether a tool-produced artifact URL may be handed to a vision model.
///
/// Audio and video artifacts still reach the client, but a model asked to
/// "look at" one either hallucinates or rejects the request outright, so they
/// must never land in a message's images.
pub fn is_vision_url(url: &str) -> bool {
    if let Some((header, _)) = url
        .strip_prefix("data:")
        .and_then(|data| data.split_once(','))
    {
        return header
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .starts_with("image/");
    }
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    !NON_VISION_EXTENSIONS
        .iter()
        .any(|extension| path.ends_with(extension))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vision_urls_exclude_audio_and_video() {
        assert!(is_vision_url("/api/artifacts/w/c/m/a.png"));
        assert!(is_vision_url("https://example.com/a.JPG"));
        assert!(is_vision_url("data:image/png;base64,AAAA"));
        assert!(is_vision_url("/api/artifacts/w/c/m/a.png?v=2"));
        for excluded in [
            "/api/artifacts/w/c/m/a.flac",
            "/api/artifacts/w/c/m/a.MP3",
            "/api/artifacts/w/c/m/a.opus",
            "/api/artifacts/w/c/m/a.wav",
            "/api/artifacts/w/c/m/a.webm",
            "/api/artifacts/w/c/m/a.mp4",
            "/api/artifacts/w/c/m/a.mkv",
            "data:audio/flac;base64,AAAA",
            "data:video/mp4;base64,AAAA",
            "data:,plain",
        ] {
            assert!(!is_vision_url(excluded), "{excluded}");
        }
    }
}
