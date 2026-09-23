use std::collections::VecDeque;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;
use std::thread;
use std::time::Duration;

use reqwest::Client;
use reqwest::redirect::Policy;
use tokio::runtime;

const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const POOL_MAX_IDLE_PER_HOST: usize = 16;
/// The fewest runtimes that keep a pool. A process holds a handful of
/// long-lived runtimes, or one per core; one that builds a runtime per task
/// would otherwise add a pool, and every connection it holds, per task for
/// the life of the process.
const MINIMUM_CAPACITY: usize = 8;

static SHARED: LazyLock<Pool> = LazyLock::new(Pool::default);

type Entries = VecDeque<(runtime::Id, Client)>;

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
/// As many runtimes keep a pool as the machine runs threads, and never fewer
/// than eight, so runtimes that take turns, one per core, do not evict one
/// another's pools. A client is built outside the lock, so a runtime that
/// already has one never waits on another's being built.
///
/// Every client refuses redirects. An endpoint that answers a completion with
/// a redirect is misconfigured, and following one would carry a vendor's key
/// header to whatever host the redirect names.
pub(crate) struct Pool {
    clients: Mutex<Entries>,
    capacity: usize,
    build: Box<dyn Fn() -> Client + Send + Sync>,
}

impl Default for Pool {
    fn default() -> Self {
        Self::new(capacity(), build)
    }
}

impl fmt::Debug for Pool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Pool")
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl Pool {
    pub(crate) fn new(capacity: usize, build: impl Fn() -> Client + Send + Sync + 'static) -> Self {
        Self {
            clients: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            build: Box::new(build),
        }
    }

    /// The pools every client in the process shares.
    pub(crate) fn shared() -> &'static Self {
        &SHARED
    }

    /// The shared pool for the current runtime, or a fresh client outside one.
    pub(crate) fn client() -> Client {
        Self::shared().for_current_runtime()
    }

    pub(crate) fn for_current_runtime(&self) -> Client {
        match runtime::Handle::try_current() {
            Ok(runtime) => self.for_runtime(runtime.id()),
            Err(_) => (self.build)(),
        }
    }

    pub(crate) fn for_runtime(&self, id: runtime::Id) -> Client {
        if let Some(client) = self.reuse(id) {
            return client;
        }

        let built = (self.build)();
        let mut clients = self.lock();
        let client = match clients.iter().position(|(owner, _)| *owner == id) {
            Some(index) => clients.remove(index).map_or(built, |(_, client)| client),
            None => built,
        };
        clients.push_front((id, client.clone()));
        clients.truncate(self.capacity);
        client
    }

    fn reuse(&self, id: runtime::Id) -> Option<Client> {
        let mut clients = self.lock();
        let index = clients.iter().position(|(owner, _)| *owner == id)?;
        if index > 0 {
            let entry = clients.remove(index)?;
            clients.push_front(entry);
        }
        clients.front().map(|(_, client)| client.clone())
    }

    fn lock(&self) -> MutexGuard<'_, Entries> {
        self.clients.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().len()
    }
}

fn capacity() -> usize {
    thread::available_parallelism()
        .map_or(MINIMUM_CAPACITY, NonZeroUsize::get)
        .max(MINIMUM_CAPACITY)
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
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use reqwest::Client;
    use tokio::runtime::Runtime;

    use super::MINIMUM_CAPACITY;
    use super::Pool;
    use super::build;

    fn runtime() -> Runtime {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
    }

    fn counted(capacity: usize) -> (Pool, Arc<AtomicUsize>) {
        let builds = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&builds);
        let pool = Pool::new(capacity, move || {
            counter.fetch_add(1, Ordering::Relaxed);
            build()
        });
        (pool, builds)
    }

    #[test]
    fn a_process_that_builds_many_runtimes_keeps_a_bounded_number_of_pools() {
        let pool = Pool::default();

        for _ in 0..pool.capacity * 4 {
            runtime().block_on(async { pool.for_current_runtime() });
        }

        assert_eq!(pool.len(), pool.capacity);
    }

    #[test]
    fn a_pool_keeps_a_client_for_every_thread_the_machine_runs() {
        let threads = thread::available_parallelism().map_or(1, usize::from);
        let capacity = Pool::default().capacity;

        assert!(capacity >= threads, "{capacity} < {threads}");
        assert!(capacity >= MINIMUM_CAPACITY, "{capacity}");
    }

    #[test]
    fn runtimes_taking_turns_within_the_capacity_never_rebuild_their_clients() {
        let (pool, builds) = counted(Pool::default().capacity);
        let runtimes: Vec<Runtime> = (0..pool.capacity).map(|_| runtime()).collect();

        for _ in 0..3 {
            for runtime in &runtimes {
                runtime.block_on(async { pool.for_current_runtime() });
            }
        }

        assert_eq!(builds.load(Ordering::Relaxed), runtimes.len());
    }

    #[test]
    fn a_runtime_reuses_its_own_pool_and_keeps_it_most_recent() {
        let (pool, builds) = counted(MINIMUM_CAPACITY);
        let kept = runtime();

        kept.block_on(async { pool.for_current_runtime() });
        for _ in 0..MINIMUM_CAPACITY - 1 {
            runtime().block_on(async { pool.for_current_runtime() });
        }
        kept.block_on(async { pool.for_current_runtime() });
        runtime().block_on(async { pool.for_current_runtime() });

        let id = kept.handle().id();
        let clients = pool.clients.lock().unwrap();
        assert_eq!(clients.len(), MINIMUM_CAPACITY);
        assert!(
            clients.iter().any(|(owner, _)| *owner == id),
            "the runtime used most recently lost its pool"
        );
        assert_eq!(builds.load(Ordering::Relaxed), MINIMUM_CAPACITY + 1);
    }

    #[test]
    fn no_runtime_gets_a_client_of_its_own_and_leaves_the_pools_alone() {
        let pool = Pool::default();
        pool.for_current_runtime();
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn a_client_is_built_without_holding_up_the_runtimes_that_already_have_one() {
        let (entered, building) = mpsc::channel::<()>();
        let (release, released) = mpsc::channel::<()>();
        let released = Mutex::new(released);
        let calls = AtomicUsize::new(0);
        let pool = Arc::new(Pool::new(MINIMUM_CAPACITY, move || {
            if calls.fetch_add(1, Ordering::Relaxed) == 1 {
                let _ = entered.send(());
                let _ = released
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10));
            }
            Client::new()
        }));
        let first = runtime();
        first.block_on(async { pool.for_current_runtime() });

        let second = thread::spawn({
            let pool = Arc::clone(&pool);
            move || runtime().block_on(async { pool.for_current_runtime() })
        });
        building
            .recv_timeout(Duration::from_secs(10))
            .expect("the second runtime started building its client");
        let (found, lookup) = mpsc::channel();
        let reuse = thread::spawn({
            let pool = Arc::clone(&pool);
            let id = first.handle().id();
            move || {
                pool.for_runtime(id);
                let _ = found.send(());
            }
        });
        let unblocked = lookup.recv_timeout(Duration::from_secs(5)).is_ok();
        release.send(()).unwrap();
        second.join().unwrap();
        reuse.join().unwrap();

        assert!(
            unblocked,
            "a runtime with a client waited for another runtime's to be built"
        );
    }
}
