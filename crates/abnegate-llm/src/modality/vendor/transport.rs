use std::time::Duration;

use reqwest::RequestBuilder;

use crate::client::Pool;
use crate::provider::ProviderError;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How a vendor client reaches its API: the shared per-runtime pool, which
/// refuses redirects so a vendor key header never follows one to another
/// host, and a deadline on every call.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Transport {
    timeout: Duration,
}

impl Default for Transport {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl Transport {
    pub(crate) fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }

    /// A `POST` to `url` on the current runtime's pool, under the deadline.
    ///
    /// The pool is looked up per call rather than kept, so a provider built on
    /// one runtime and used on another never borrows a dead connection.
    pub(crate) fn post(&self, url: impl reqwest::IntoUrl) -> RequestBuilder {
        Pool::client().post(url).timeout(self.timeout)
    }

    /// Send `request` and read a JSON body, reporting a refusal with its
    /// status and redacted body.
    pub(crate) async fn send(
        &self,
        request: RequestBuilder,
    ) -> Result<serde_json::Value, ProviderError> {
        let response = request.send().await.map_err(|error| self.failure(error))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".into());
            return Err(ProviderError::api(status, message));
        }

        let body = response
            .bytes()
            .await
            .map_err(|error| self.failure(error))?;
        serde_json::from_slice(&body).map_err(ProviderError::parse)
    }

    fn failure(&self, error: reqwest::Error) -> ProviderError {
        if error.is_timeout() {
            return ProviderError::network(format!("no answer within {:?}", self.timeout));
        }
        ProviderError::network(error.without_url())
    }
}
