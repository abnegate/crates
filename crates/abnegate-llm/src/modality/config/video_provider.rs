use abnegate_secret::SecretValue;
use serde::{Deserialize, Serialize};

/// Which video provider to use, and how to reach it.
///
/// The key is never serialised, so a config written back to disk does not
/// carry it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct VideoProviderConfig {
    pub provider: String,
    #[serde(skip_serializing)]
    pub api_key: Option<SecretValue>,
    pub base_url: Option<String>,
}

impl VideoProviderConfig {
    /// A config naming `provider`, with nothing else set.
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            api_key: None,
            base_url: None,
        }
    }

    /// Authenticate with `api_key`.
    pub fn with_api_key(mut self, api_key: impl Into<SecretValue>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Reach the provider at `base_url` rather than its public endpoint.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }
}
