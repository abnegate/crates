use serde::{Deserialize, Serialize};

/// Audio an [`AudioProvider`](crate::AudioProvider) or a
/// [`VoiceProvider`](crate::VoiceProvider) made.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AudioResponse {
    pub data: Vec<u8>,
    pub format: String,
    pub duration_seconds: f64,
    pub sample_rate: u32,
}

impl AudioResponse {
    /// `duration_seconds` of audio encoded as `format`, such as `wav`, at
    /// `sample_rate` samples a second.
    pub fn new(
        data: Vec<u8>,
        format: impl Into<String>,
        duration_seconds: f64,
        sample_rate: u32,
    ) -> Self {
        Self {
            data,
            format: format.into(),
            duration_seconds,
            sample_rate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let response = AudioResponse::new(vec![255, 128, 0], "wav", 10.5, 48_000);

        let json = serde_json::to_string(&response).unwrap();
        let roundtrip: AudioResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.data, vec![255, 128, 0]);
        assert_eq!(roundtrip.format, "wav");
        assert!((roundtrip.duration_seconds - 10.5).abs() < f64::EPSILON);
        assert_eq!(roundtrip.sample_rate, 48_000);
    }
}
