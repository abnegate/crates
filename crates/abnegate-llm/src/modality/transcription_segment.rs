use serde::Deserialize;
use serde::Serialize;

/// One timed piece of a transcription.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TranscriptionSegment {
    /// Where the piece starts, in seconds into the audio.
    pub start: f64,
    /// Where the piece ends, in seconds into the audio.
    pub end: f64,
    /// What was heard.
    pub text: String,
    /// How sure the provider is of it, from 0 to 1.
    pub confidence: f64,
}

impl TranscriptionSegment {
    /// `text` heard from `start` to `end` seconds into the audio, with the
    /// provider's `confidence` in it.
    pub fn new(start: f64, end: f64, text: impl Into<String>, confidence: f64) -> Self {
        Self {
            start,
            end,
            text: text.into(),
            confidence,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let segment = TranscriptionSegment::new(1.5, 3.0, "Hello world", 0.98);

        let json = serde_json::to_string(&segment).unwrap();
        let roundtrip: TranscriptionSegment = serde_json::from_str(&json).unwrap();

        assert!((roundtrip.start - 1.5).abs() < f64::EPSILON);
        assert!((roundtrip.end - 3.0).abs() < f64::EPSILON);
        assert_eq!(roundtrip.text, "Hello world");
        assert!((roundtrip.confidence - 0.98).abs() < f64::EPSILON);
    }
}
