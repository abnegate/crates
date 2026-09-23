use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceRequest {
    pub text: String,
    pub voice_id: Option<String>,
    pub voice_description: Option<String>,
    pub emotion: Option<String>,
    pub speed: f64,
    pub reference_samples: Vec<String>,
}

impl VoiceRequest {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            voice_id: None,
            voice_description: None,
            emotion: None,
            speed: 1.0,
            reference_samples: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_fills_in_the_defaults() {
        let request = VoiceRequest::new("Hello there");
        assert_eq!(request.text, "Hello there");
        assert!((request.speed - 1.0).abs() < f64::EPSILON);
        assert!(request.voice_id.is_none());
        assert!(request.voice_description.is_none());
        assert!(request.emotion.is_none());
        assert!(request.reference_samples.is_empty());
    }

    #[test]
    fn round_trips_with_every_option_set() {
        let request = VoiceRequest {
            text: "Welcome to the adventure!".into(),
            voice_id: Some("voice_abc".into()),
            voice_description: Some("Deep male narrator".into()),
            emotion: Some("excited".into()),
            speed: 1.1,
            reference_samples: vec!["sample1.wav".into()],
        };

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: VoiceRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.text, "Welcome to the adventure!");
        assert_eq!(roundtrip.voice_id.as_deref(), Some("voice_abc"));
        assert_eq!(
            roundtrip.voice_description.as_deref(),
            Some("Deep male narrator")
        );
        assert_eq!(roundtrip.emotion.as_deref(), Some("excited"));
        assert!((roundtrip.speed - 1.1).abs() < f64::EPSILON);
        assert_eq!(roundtrip.reference_samples.len(), 1);
    }

    #[test]
    fn empty_text_survives_a_round_trip() {
        let request = VoiceRequest::new("");
        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: VoiceRequest = serde_json::from_str(&json).unwrap();
        assert!(roundtrip.text.is_empty());
    }
}
