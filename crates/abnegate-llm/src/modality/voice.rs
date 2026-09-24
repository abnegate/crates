use serde::{Deserialize, Serialize};

/// A voice a [`VoiceProvider`](crate::VoiceProvider) can speak in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Voice {
    /// The provider's identifier, which a [`VoiceRequest`](crate::VoiceRequest)
    /// names as its `voice_id`. Serialised as `voice_id`.
    #[serde(rename = "voice_id")]
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub preview_url: Option<String>,
    pub labels: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_every_field_set() {
        let voice = Voice {
            id: "abc123".into(),
            name: "Rachel".into(),
            description: Some("A warm female voice".into()),
            preview_url: Some("https://example.com/preview.mp3".into()),
            labels: vec!["female".into(), "warm".into()],
        };

        let json = serde_json::to_string(&voice).unwrap();
        let roundtrip: Voice = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.id, "abc123");
        assert_eq!(roundtrip.name, "Rachel");
        assert_eq!(roundtrip.labels.len(), 2);
    }

    #[test]
    fn round_trips_with_the_optional_fields_empty() {
        let voice = Voice {
            id: "v1".into(),
            name: "Test".into(),
            description: None,
            preview_url: None,
            labels: Vec::new(),
        };

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
