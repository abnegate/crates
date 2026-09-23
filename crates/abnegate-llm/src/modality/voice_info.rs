use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceInfo {
    pub voice_id: String,
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
        let info = VoiceInfo {
            voice_id: "abc123".into(),
            name: "Rachel".into(),
            description: Some("A warm female voice".into()),
            preview_url: Some("https://example.com/preview.mp3".into()),
            labels: vec!["female".into(), "warm".into()],
        };

        let json = serde_json::to_string(&info).unwrap();
        let roundtrip: VoiceInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.voice_id, "abc123");
        assert_eq!(roundtrip.name, "Rachel");
        assert_eq!(roundtrip.labels.len(), 2);
    }

    #[test]
    fn round_trips_with_the_optional_fields_empty() {
        let info = VoiceInfo {
            voice_id: "v1".into(),
            name: "Test".into(),
            description: None,
            preview_url: None,
            labels: Vec::new(),
        };

        let json = serde_json::to_string(&info).unwrap();
        let roundtrip: VoiceInfo = serde_json::from_str(&json).unwrap();

        assert!(roundtrip.labels.is_empty());
        assert!(roundtrip.description.is_none());
        assert!(roundtrip.preview_url.is_none());
    }
}
