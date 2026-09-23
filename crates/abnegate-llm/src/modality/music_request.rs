use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicRequest {
    pub prompt: String,
    pub duration_seconds: f64,
    pub genre: Option<String>,
    pub mood: Option<String>,
    pub tempo_bpm: Option<u32>,
    pub reference_audio: Option<String>,
}

impl MusicRequest {
    pub fn new(prompt: impl Into<String>, duration_seconds: f64) -> Self {
        Self {
            prompt: prompt.into(),
            duration_seconds,
            genre: None,
            mood: None,
            tempo_bpm: None,
            reference_audio: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_fills_in_the_defaults() {
        let request = MusicRequest::new("Calm ambient", 30.0);
        assert_eq!(request.prompt, "Calm ambient");
        assert!((request.duration_seconds - 30.0).abs() < f64::EPSILON);
        assert!(request.genre.is_none());
        assert!(request.mood.is_none());
        assert!(request.tempo_bpm.is_none());
        assert!(request.reference_audio.is_none());
    }

    #[test]
    fn round_trips_with_every_option_set() {
        let request = MusicRequest {
            prompt: "Epic orchestral battle theme".into(),
            duration_seconds: 120.0,
            genre: Some("orchestral".into()),
            mood: Some("intense".into()),
            tempo_bpm: Some(140),
            reference_audio: Some("reference.mp3".into()),
        };

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: MusicRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.prompt, "Epic orchestral battle theme");
        assert!((roundtrip.duration_seconds - 120.0).abs() < f64::EPSILON);
        assert_eq!(roundtrip.genre.as_deref(), Some("orchestral"));
        assert_eq!(roundtrip.mood.as_deref(), Some("intense"));
        assert_eq!(roundtrip.tempo_bpm, Some(140));
        assert_eq!(roundtrip.reference_audio.as_deref(), Some("reference.mp3"));
    }

    #[test]
    fn a_zero_duration_survives_a_round_trip() {
        let request = MusicRequest::new("Epic battle", 0.0);
        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: MusicRequest = serde_json::from_str(&json).unwrap();
        assert!((roundtrip.duration_seconds - 0.0).abs() < f64::EPSILON);
    }
}
