use std::collections::VecDeque;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::Duration;

use reqwest::Client;
use reqwest::redirect::Policy;
use tokio::runtime;

const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const POOL_MAX_IDLE_PER_HOST: usize = 16;
/// How many runtimes keep a pool. A process holds a handful of long-lived
/// runtimes; one that builds a runtime per task would otherwise add a pool,
/// and every connection it holds, per task for the life of the process.
const CAPACITY: usize = 8;

static SHARED: LazyLock<Pool> = LazyLock::new(Pool::default);

/// Connection pools held per runtime, most recently used first.
///
/// Building a `reqwest::Client` per turn throws away TLS sessions and
/// keep-alives to the endpoint, which is the whole time-to-first-token budget
/// on a local model, so completions share one.
///
/// They cannot share more widely than the runtime. Every pooled connection is
/// driven by a task belonging to the runtime that opened it, so a pool reused
/// from a second runtime hands out connections whose driver died with the
/// first, and the send fails with "runtime dropped the dispatch task" without
/// ever reaching the server.
///
/// Every client refuses redirects. An endpoint that answers a completion with
/// a redirect is misconfigured, and following one would carry a vendor's key
/// header to whatever host the redirect names.
#[derive(Default)]
pub(crate) struct Pool {
    clients: Mutex<VecDeque<(runtime::Id, Client)>>,
}

impl Pool {
    /// The pool for the current runtime, or a fresh client outside one.
    pub(crate) fn client() -> Client {
        SHARED.for_current_runtime()
    }

    fn for_current_runtime(&self) -> Client {
        let Ok(runtime) = runtime::Handle::try_current() else {
            return build();
        };
        let id = runtime.id();
        let mut clients = self
            .clients
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let client = match clients.iter().position(|(owner, _)| *owner == id) {
            Some(index) => clients
                .remove(index)
                .map_or_else(build, |(_, client)| client),
            None => build(),
        };
        clients.push_front((id, client.clone()));
        clients.truncate(CAPACITY);
        client
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.clients
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

fn build() -> Client {
    Client::builder()
        .pool_max_idle_per_host(POOL_MAX_IDLE_PER_HOST)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(Policy::none())
        .tcp_nodelay(true)
        .build()
        .unwrap_or_else(|_| Client::new())
}

#[cfg(test)]
mod tests {
    use super::CAPACITY;
    use super::Pool;

    #[test]
    fn a_process_that_builds_many_runtimes_keeps_a_bounded_number_of_pools() {
        let pool = Pool::default();

        for _ in 0..CAPACITY * 4 {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(async { pool.for_current_runtime() });
        }

        assert_eq!(pool.len(), CAPACITY);
    }

    #[test]
    fn a_runtime_reuses_its_own_pool_and_keeps_it_most_recent() {
        let pool = Pool::default();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        runtime.block_on(async { pool.for_current_runtime() });
        for _ in 0..CAPACITY - 1 {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(async { pool.for_current_runtime() });
        }
        runtime.block_on(async { pool.for_current_runtime() });
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async { pool.for_current_runtime() });

        let id = runtime.handle().id();
        let clients = pool.clients.lock().unwrap();
        assert_eq!(clients.len(), CAPACITY);
        assert!(
            clients.iter().any(|(owner, _)| *owner == id),
            "the runtime used most recently lost its pool"
        );
    }

    #[test]
    fn no_runtime_gets_a_client_of_its_own_and_leaves_the_pools_alone() {
        let pool = Pool::default();
        pool.for_current_runtime();
        assert_eq!(pool.len(), 0);
    }
}
