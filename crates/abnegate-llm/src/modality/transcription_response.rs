use serde::{Deserialize, Serialize};

use crate::modality::TranscriptionSegment;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResponse {
    pub text: String,
    pub segments: Vec<TranscriptionSegment>,
    pub language: String,
    pub duration_seconds: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let response = TranscriptionResponse {
            text: "Hello world".into(),
            segments: vec![TranscriptionSegment {
                start: 0.0,
                end: 1.5,
                text: "Hello world".into(),
                confidence: 0.95,
            }],
            language: "en".into(),
            duration_seconds: 1.5,
        };

        let json = serde_json::to_string(&response).unwrap();
        let roundtrip: TranscriptionResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.text, "Hello world");
        assert_eq!(roundtrip.language, "en");
        assert_eq!(roundtrip.segments.len(), 1);
        assert!((roundtrip.segments[0].confidence - 0.95).abs() < f64::EPSILON);
    }
}
