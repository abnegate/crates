use serde::{Deserialize, Serialize};

const DEFAULT_SPEED: f64 = 1.0;

/// Text for a [`VoiceProvider`](crate::VoiceProvider) to speak.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct VoiceRequest {
    pub text: String,
    pub voice_id: Option<String>,
    pub voice_description: Option<String>,
    pub emotion: Option<String>,
    pub speed: f64,
    pub reference_samples: Vec<String>,
}

impl VoiceRequest {
    /// `text` in the provider's default voice at normal speed.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            voice_id: None,
            voice_description: None,
            emotion: None,
            speed: DEFAULT_SPEED,
            reference_samples: Vec::new(),
        }
    }

    /// Speak in the voice the provider knows as `voice_id`.
    pub fn with_voice_id(mut self, voice_id: impl Into<String>) -> Self {
        self.voice_id = Some(voice_id.into());
        self
    }

    /// Describe the voice to speak in, for a provider that designs one.
    pub fn with_voice_description(mut self, voice_description: impl Into<String>) -> Self {
        self.voice_description = Some(voice_description.into());
        self
    }

    /// Set [`Self::emotion`].
    pub fn with_emotion(mut self, emotion: impl Into<String>) -> Self {
        self.emotion = Some(emotion.into());
        self
    }

    /// Set [`Self::speed`], where 1.0 is normal.
    pub fn with_speed(mut self, speed: f64) -> Self {
        self.speed = speed;
        self
    }

    /// Set the recordings the voice should sound like.
    pub fn with_reference_samples(mut self, reference_samples: Vec<String>) -> Self {
        self.reference_samples = reference_samples;
        self
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
        let request = VoiceRequest::new("Welcome to the adventure!")
            .with_voice_id("voice_abc")
            .with_voice_description("Deep male narrator")
            .with_emotion("excited")
            .with_speed(1.1)
            .with_reference_samples(vec!["sample1.wav".into()]);

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
