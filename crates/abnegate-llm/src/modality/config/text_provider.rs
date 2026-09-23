use abnegate_secret::SecretValue;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextProviderConfig {
    pub provider: String,
    #[serde(skip_serializing)]
    pub api_key: Option<SecretValue>,
    #[serde(skip_serializing)]
    pub oauth_token: Option<SecretValue>,
    pub model: Option<String>,
    pub base_url: Option<String>,
}
