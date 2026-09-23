use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;
use std::time::Duration;

use reqwest::Client;
use reqwest::RequestBuilder;
use tokio::runtime;

use crate::client::Pool;
use crate::provider::ProviderError;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How a vendor client reaches its API: the shared per-runtime pool, which
/// refuses redirects so a vendor key header never follows one to another
/// host, and a deadline on every call.
///
/// The client for the runtime of the last call is kept, and the shared pool
/// is asked again only when a call comes from another runtime, so a provider
/// built on one runtime and used on another never borrows a dead connection.
#[derive(Debug, Clone)]
pub(crate) struct Transport {
    timeout: Duration,
    pool: &'static Pool,
    client: Arc<Mutex<Option<(runtime::Id, Client)>>>,
}

impl Default for Transport {
    fn default() -> Self {
        Self::with_timeout(DEFAULT_TIMEOUT)
    }
}

impl Transport {
    pub(crate) fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout,
            pool: Pool::shared(),
            client: Arc::default(),
        }
    }

    /// A `POST` to `url` on the current runtime's pool, under the deadline.
    pub(crate) fn post(&self, url: impl reqwest::IntoUrl) -> RequestBuilder {
        self.client().post(url).timeout(self.timeout)
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

    fn client(&self) -> Client {
        let Ok(runtime) = runtime::Handle::try_current() else {
            return self.pool.for_current_runtime();
        };
        let id = runtime.id();
        if let Some((owner, client)) = self.cached().as_ref()
            && *owner == id
        {
            return client.clone();
        }

        let client = self.pool.for_runtime(id);
        *self.cached() = Some((id, client.clone()));
        client
    }

    fn cached(&self) -> MutexGuard<'_, Option<(runtime::Id, Client)>> {
        self.client.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn failure(&self, error: reqwest::Error) -> ProviderError {
        if error.is_timeout() {
            return ProviderError::network(format!("no answer within {:?}", self.timeout));
        }
        ProviderError::network(error.without_url())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use reqwest::Client;
    use tokio::runtime::Runtime;

    use super::*;

    const CAPACITY: usize = 2;
    const RUNTIMES: usize = 6;

    #[test]
    fn transports_on_more_runtimes_than_the_pool_holds_keep_their_clients() {
        let builds = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&builds);
        let pool: &'static Pool = Box::leak(Box::new(Pool::new(CAPACITY, move || {
            counter.fetch_add(1, Ordering::Relaxed);
            Client::new()
        })));
        let runtimes: Vec<Runtime> = (0..RUNTIMES)
            .map(|_| {
                tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap()
            })
            .collect();
        let transports: Vec<Transport> = runtimes
            .iter()
            .map(|_| Transport {
                pool,
                ..Transport::default()
            })
            .collect();

        for _ in 0..4 {
            for (runtime, transport) in runtimes.iter().zip(&transports) {
                runtime.block_on(async { transport.client() });
            }
        }

        assert_eq!(builds.load(Ordering::Relaxed), RUNTIMES);
    }

    #[test]
    fn a_transport_moved_to_another_runtime_takes_the_client_of_that_runtime() {
        let builds = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&builds);
        let pool: &'static Pool = Box::leak(Box::new(Pool::new(CAPACITY, move || {
            counter.fetch_add(1, Ordering::Relaxed);
            Client::new()
        })));
        let transport = Transport {
            pool,
            ..Transport::default()
        };
        let runtime = || {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
        };
        let first = runtime();
        let second = runtime();

        first.block_on(async { transport.client() });
        second.block_on(async { transport.client() });

        let cached = transport.cached();
        assert_eq!(
            cached.as_ref().map(|(owner, _)| *owner),
            Some(second.handle().id())
        );
        assert_eq!(builds.load(Ordering::Relaxed), 2);
    }
}
