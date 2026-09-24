use serde::{Deserialize, Serialize};

/// A sound effect for an [`AudioProvider`](crate::AudioProvider) to make.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SoundEffectRequest {
    pub prompt: String,
    pub duration_seconds: f64,
    pub category: Option<String>,
}

impl SoundEffectRequest {
    /// `duration_seconds` of the sound `prompt` describes, in no category.
    pub fn new(prompt: impl Into<String>, duration_seconds: f64) -> Self {
        Self {
            prompt: prompt.into(),
            duration_seconds,
            category: None,
        }
    }

    /// Set [`Self::category`].
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let request = SoundEffectRequest::new("Explosion", 2.5).with_category("combat");

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: SoundEffectRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.prompt, "Explosion");
        assert!((roundtrip.duration_seconds - 2.5).abs() < f64::EPSILON);
        assert_eq!(roundtrip.category.as_deref(), Some("combat"));
    }
}
