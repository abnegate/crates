use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionSegment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub confidence: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let segment = TranscriptionSegment {
            start: 1.5,
            end: 3.0,
            text: "Hello world".into(),
            confidence: 0.98,
        };

        let json = serde_json::to_string(&segment).unwrap();
        let roundtrip: TranscriptionSegment = serde_json::from_str(&json).unwrap();

        assert!((roundtrip.start - 1.5).abs() < f64::EPSILON);
        assert!((roundtrip.end - 3.0).abs() < f64::EPSILON);
        assert_eq!(roundtrip.text, "Hello world");
    }
}
