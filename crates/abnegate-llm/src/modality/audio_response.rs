use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioResponse {
    pub data: Vec<u8>,
    pub format: String,
    pub duration_seconds: f64,
    pub sample_rate: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let response = AudioResponse {
            data: vec![255, 128, 0],
            format: "wav".into(),
            duration_seconds: 10.5,
            sample_rate: 48_000,
        };

        let json = serde_json::to_string(&response).unwrap();
        let roundtrip: AudioResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.data, vec![255, 128, 0]);
        assert_eq!(roundtrip.format, "wav");
        assert!((roundtrip.duration_seconds - 10.5).abs() < f64::EPSILON);
        assert_eq!(roundtrip.sample_rate, 48_000);
    }
}
