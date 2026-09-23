use chrono::DateTime;
use chrono::TimeDelta;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

/// What is known about a stored token besides the token itself.
///
/// The expiry is stored as integer Unix seconds, the shape entries already in
/// the keyring were written in.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TokenMetadata {
    pub host: String,
    #[serde(with = "chrono::serde::ts_seconds")]
    pub expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl TokenMetadata {
    pub fn new(host: impl Into<String>, expires_at: DateTime<Utc>) -> Self {
        Self {
            host: host.into(),
            expires_at,
            user_id: None,
            email: None,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.expires_within(TimeDelta::zero())
    }

    /// Whether the token is already gone or will be within `leeway`, which is
    /// the question worth asking before spending a round trip on it.
    pub fn expires_within(&self, leeway: TimeDelta) -> bool {
        self.expires_at.signed_duration_since(Utc::now()) <= leeway
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> TokenMetadata {
        TokenMetadata {
            host: "https://api.example.com".to_string(),
            expires_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            user_id: Some("user-123".to_string()),
            email: Some("person@example.com".to_string()),
        }
    }

    #[test]
    fn metadata_serializes() {
        let json = serde_json::to_string(&metadata()).unwrap();

        assert!(json.contains("api.example.com"));
        assert!(json.contains("\"expires_at\":1700000000"), "{json}");
        assert!(json.contains("user-123"));
        assert!(json.contains("person@example.com"));
    }

    #[test]
    fn an_entry_written_by_the_zone_cli_still_reads() {
        let restored: TokenMetadata = serde_json::from_str(
            r#"{
            "host": "https://api.zone.io",
            "expires_at": 1800000000,
            "user_id": "abc-456",
            "email": "user@zone.io"
        }"#,
        )
        .unwrap();

        assert_eq!(restored.host, "https://api.zone.io");
        assert_eq!(restored.expires_at.timestamp(), 1_800_000_000);
        assert_eq!(restored.user_id, Some("abc-456".to_string()));
        assert_eq!(restored.email, Some("user@zone.io".to_string()));
    }

    #[test]
    fn an_expiry_is_written_as_unix_seconds() {
        let json = serde_json::to_value(TokenMetadata::new(
            "https://api.example.com",
            DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
        ))
        .unwrap();

        assert_eq!(json["expires_at"], serde_json::json!(1_800_000_000));
    }

    #[test]
    fn an_identity_is_optional() {
        let restored: TokenMetadata = serde_json::from_str(
            r#"{ "host": "https://api.example.com", "expires_at": 1800000000 }"#,
        )
        .unwrap();

        assert!(restored.user_id.is_none());
        assert!(restored.email.is_none());
        assert!(
            !serde_json::to_string(&restored)
                .unwrap()
                .contains("user_id")
        );
    }

    #[test]
    fn metadata_round_trips() {
        let original = metadata();

        let restored: TokenMetadata =
            serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();

        assert_eq!(restored, original);
    }

    #[test]
    fn metadata_clones() {
        let original = metadata();

        assert_eq!(original.clone(), original);
    }

    #[test]
    fn metadata_is_debuggable() {
        let debugged = format!("{:?}", metadata());

        assert!(debugged.contains("TokenMetadata"));
        assert!(debugged.contains("api.example.com"));
    }

    #[test]
    fn every_host_round_trips() {
        let hosts = [
            "http://localhost:8000",
            "https://api.example.com",
            "https://api.example.com:443",
            "http://192.168.1.100:3000",
        ];

        for host in hosts {
            let original = TokenMetadata::new(host, Utc::now());

            let restored: TokenMetadata =
                serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();

            assert_eq!(restored.host, host);
        }
    }

    #[test]
    fn every_expiry_round_trips() {
        for timestamp in [0_i64, 1_000_000_000, 1_700_000_000, 2_000_000_000] {
            let original = TokenMetadata::new(
                "https://api.example.com",
                DateTime::from_timestamp(timestamp, 0).unwrap(),
            );

            let restored: TokenMetadata =
                serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();

            assert_eq!(restored.expires_at.timestamp(), timestamp);
        }
    }

    #[test]
    fn a_past_expiry_has_expired() {
        let expired =
            TokenMetadata::new("https://api.example.com", Utc::now() - TimeDelta::hours(1));

        assert!(expired.is_expired());
    }

    #[test]
    fn a_future_expiry_has_not() {
        let live = TokenMetadata::new("https://api.example.com", Utc::now() + TimeDelta::hours(1));

        assert!(!live.is_expired());
    }

    #[test]
    fn a_token_about_to_expire_counts_as_expiring() {
        let live = TokenMetadata::new(
            "https://api.example.com",
            Utc::now() + TimeDelta::seconds(30),
        );

        assert!(live.expires_within(TimeDelta::seconds(60)));
        assert!(!live.expires_within(TimeDelta::seconds(5)));
    }

    #[test]
    fn a_new_token_carries_no_identity() {
        let metadata = TokenMetadata::new("https://api.example.com", Utc::now());

        assert!(metadata.user_id.is_none());
        assert!(metadata.email.is_none());
    }
}
