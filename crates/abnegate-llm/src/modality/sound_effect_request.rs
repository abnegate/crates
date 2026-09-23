use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundEffectRequest {
    pub prompt: String,
    pub duration_seconds: f64,
    pub category: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let request = SoundEffectRequest {
            prompt: "Explosion".into(),
            duration_seconds: 2.5,
            category: Some("combat".into()),
        };

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: SoundEffectRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.prompt, "Explosion");
        assert!((roundtrip.duration_seconds - 2.5).abs() < f64::EPSILON);
        assert_eq!(roundtrip.category.as_deref(), Some("combat"));
    }
}
