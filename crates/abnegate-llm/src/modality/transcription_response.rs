use serde::{Deserialize, Serialize};

use crate::modality::TranscriptionSegment;

/// What a [`TranscriptionProvider`](crate::TranscriptionProvider) heard.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TranscriptionResponse {
    pub text: String,
    pub segments: Vec<TranscriptionSegment>,
    pub language: String,
    pub duration_seconds: f64,
}

impl TranscriptionResponse {
    /// `text` heard in `language` over `duration_seconds` of audio, with no
    /// timed segments.
    pub fn new(
        text: impl Into<String>,
        language: impl Into<String>,
        duration_seconds: f64,
    ) -> Self {
        Self {
            text: text.into(),
            segments: Vec::new(),
            language: language.into(),
            duration_seconds,
        }
    }

    /// Set the timed pieces the text is made of.
    pub fn with_segments(mut self, segments: Vec<TranscriptionSegment>) -> Self {
        self.segments = segments;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let response = TranscriptionResponse::new("Hello world", "en", 1.5).with_segments(vec![
            TranscriptionSegment::new(0.0, 1.5, "Hello world", 0.95),
        ]);

        let json = serde_json::to_string(&response).unwrap();
        let roundtrip: TranscriptionResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.text, "Hello world");
        assert_eq!(roundtrip.language, "en");
        assert!((roundtrip.duration_seconds - 1.5).abs() < f64::EPSILON);
        assert_eq!(roundtrip.segments.len(), 1);
        assert!((roundtrip.segments[0].confidence - 0.95).abs() < f64::EPSILON);
    }
}
