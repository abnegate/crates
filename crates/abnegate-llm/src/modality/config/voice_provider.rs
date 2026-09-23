use abnegate_secret::SecretValue;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceProviderConfig {
    pub provider: String,
    #[serde(skip_serializing)]
    pub api_key: Option<SecretValue>,
    pub base_url: Option<String>,
}
