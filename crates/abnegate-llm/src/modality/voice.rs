use serde::{Deserialize, Serialize};

/// A voice a [`VoiceProvider`](crate::VoiceProvider) can speak in.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Voice {
    /// The provider's identifier, which a [`VoiceRequest`](crate::VoiceRequest)
    /// names as its `voice_id`. Serialised as `voice_id`.
    #[serde(rename = "voice_id")]
    pub id: String,
    /// The name the provider shows for the voice.
    pub name: String,
    /// The provider's description of how the voice sounds, if it gives one.
    pub description: Option<String>,
    /// Where a sample of the voice can be heard, if the provider offers one.
    pub preview_url: Option<String>,
    /// The provider's tags for the voice, such as `female` or `warm`.
    pub labels: Vec<String>,
}

impl Voice {
    /// The voice the provider knows as `id`, called `name`, with no
    /// description, preview or labels.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            preview_url: None,
            labels: Vec::new(),
        }
    }

    /// Set [`Self::description`].
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set where a sample of the voice can be heard.
    pub fn with_preview_url(mut self, preview_url: impl Into<String>) -> Self {
        self.preview_url = Some(preview_url.into());
        self
    }

    /// Set the provider's tags for the voice.
    pub fn with_labels(mut self, labels: Vec<String>) -> Self {
        self.labels = labels;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_every_field_set() {
        let voice = Voice::new("abc123", "Rachel")
            .with_description("A warm female voice")
            .with_preview_url("https://example.com/preview.mp3")
            .with_labels(vec!["female".into(), "warm".into()]);

        let json = serde_json::to_string(&voice).unwrap();
        let roundtrip: Voice = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.id, "abc123");
        assert_eq!(roundtrip.name, "Rachel");
        assert_eq!(
            roundtrip.description.as_deref(),
            Some("A warm female voice")
        );
        assert_eq!(
            roundtrip.preview_url.as_deref(),
            Some("https://example.com/preview.mp3")
        );
        assert_eq!(roundtrip.labels.len(), 2);
    }

    #[test]
    fn round_trips_with_the_optional_fields_empty() {
        let voice = Voice::new("v1", "Test");

        let json = serde_json::to_string(&voice).unwrap();
        let roundtrip: Voice = serde_json::from_str(&json).unwrap();

        assert!(roundtrip.labels.is_empty());
        assert!(roundtrip.description.is_none());
        assert!(roundtrip.preview_url.is_none());
    }

    #[test]
    fn the_identifier_keeps_its_serialised_name() {
        let saved =
            r#"{"voice_id":"v1","name":"Test","description":null,"preview_url":null,"labels":[]}"#;

        let voice: Voice = serde_json::from_str(saved).unwrap();

        assert_eq!(voice.id, "v1");
        assert_eq!(serde_json::to_string(&voice).unwrap(), saved);
    }
}
