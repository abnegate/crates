use abnegate_secret::SecretValue;
use serde::{Deserialize, Serialize};

/// Which image provider to use, and how to reach it.
///
/// The key is never serialised, so a config written back to disk does not
/// carry it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ImageProviderConfig {
    pub provider: String,
    #[serde(skip_serializing)]
    pub api_key: Option<SecretValue>,
    pub model: Option<String>,
    pub base_url: Option<String>,
}

impl ImageProviderConfig {
    /// A config naming `provider`, with nothing else set.
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            api_key: None,
            model: None,
            base_url: None,
        }
    }

    /// Authenticate with `api_key`.
    pub fn with_api_key(mut self, api_key: impl Into<SecretValue>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Ask for `model` rather than the provider's default.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Reach the provider at `base_url` rather than its public endpoint.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }
}
