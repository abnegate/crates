use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::warn;

use super::ChatError;
use super::ContextStore;
use super::Lease;

/// An independently scheduled renewal, unaffected by blocked websocket sends or tools.
/// Dropping the guard stops renewal; callers release explicitly after durable completion.
pub struct Guard {
    lease: Lease,
    lost: watch::Receiver<bool>,
    task: Option<JoinHandle<()>>,
}

impl Guard {
    pub async fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
    pub fn lease(&self) -> &Lease {
        &self.lease
    }
    pub fn is_lost(&self) -> bool {
        *self.lost.borrow()
    }
    pub async fn lost(&mut self) {
        if self.is_lost() {
            return;
        }
        let _ = self.lost.wait_for(|lost| *lost).await;
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Renewals scheduled per lifetime, so a lease outlives losing some of them.
const RENEWALS_PER_LIFETIME: u32 = 3;

/// How long to wait before retrying a renewal that failed for a reason other
/// than the lease being gone, short enough that what the lease has left holds
/// many attempts.
const RETRY_DELAY: Duration = Duration::from_secs(1);

fn remaining(lease: &Lease) -> Duration {
    (lease.expires_at - Utc::now()).to_std().unwrap_or_default()
}

/// Renew until it succeeds or the lease is provably gone.
///
/// Anything other than [`ChatError::LeaseLost`] leaves the row untouched, so the
/// `expires_at` already granted still stands and retrying inside it reclaims a
/// lease nobody else can hold; each attempt is cut short at that instant so a
/// renewal cannot outlive the lease it is renewing.
async fn renew(
    store: &dyn ContextStore,
    lease: &Lease,
    lifetime: Duration,
) -> Result<Lease, ChatError> {
    loop {
        let left = remaining(lease);
        if left.is_zero() {
            let error = ChatError::LeaseLost;
            warn!(chat = %lease.chat_id, owner = %lease.owner, fence = lease.fence, %error,
                "Chat lease expired before a renewal succeeded");
            return Err(error);
        }
        let error = match tokio::time::timeout(left, store.renew(lease, lifetime)).await {
            Ok(Ok(renewed)) => return Ok(renewed),
            Ok(Err(error)) => error,
            Err(elapsed) => ChatError::Backend(elapsed.to_string()),
        };
        if matches!(error, ChatError::LeaseLost) {
            warn!(chat = %lease.chat_id, owner = %lease.owner, fence = lease.fence, %error,
                "Chat lease was taken over or expired; the response will stop");
            return Err(error);
        }
        warn!(chat = %lease.chat_id, owner = %lease.owner, fence = lease.fence, %error,
            "Chat lease renewal failed; retrying while the lease holds");
        tokio::time::sleep(RETRY_DELAY.min(remaining(lease))).await;
    }
}

/// Renew a lease on its own schedule, so a blocked websocket send or a slow
/// tool cannot let it lapse mid-turn. Dropping the guard stops renewal;
/// callers still release explicitly once the turn is durable.
///
/// A renewal that fails without losing the lease is retried for as long as the
/// lease has left, so a stalled database call costs a turn only once nobody
/// could have saved it.
pub fn keep_alive(
    store: Arc<dyn ContextStore>,
    lease: Lease,
    lifetime: Duration,
) -> Result<Guard, ChatError> {
    if lifetime.is_zero() {
        return Err(ChatError::Integrity(
            "lease lifetime must be non-zero".into(),
        ));
    }
    let (tx, lost) = watch::channel(false);
    let mut current = lease.clone();
    let task = tokio::spawn(async move {
        let interval = lifetime / RENEWALS_PER_LIFETIME;
        loop {
            tokio::time::sleep(interval).await;
            match renew(store.as_ref(), &current, lifetime).await {
                Ok(next) => current = next,
                Err(_) => {
                    let _ = tx.send(true);
                    return;
                }
            }
        }
    });
    Ok(Guard {
        lease,
        lost,
        task: Some(task),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::future::pending;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use chrono::TimeDelta;
    use serde_json::Value;
    use tokio::time::Instant;
    use uuid::Uuid;

    use super::*;
    use crate::chat::Evidence;
    use crate::chat::History;
    use crate::chat::NewEntry;
    use crate::chat::ReplayMessage;
    use crate::chat::StoredMessage;
    use crate::chat::Summary;
    use crate::test_support::captured_logs;

    const LIFETIME: Duration = Duration::from_secs(30);
    /// Longer than any timeline here, so a loop that stops making progress
    /// fails with its own message instead of hanging the suite.
    const WATCHDOG: Duration = Duration::from_secs(600);
    const POOL_TIMED_OUT: &str = "pool timed out";

    /// What the next renewal does.
    enum Renewal {
        Granted,
        Transient,
        Lost,
        Hang,
    }

    /// A store that only answers renewals, from a script. Every other method is
    /// unreachable: the keep-alive calls nothing else.
    struct Fake {
        script: Mutex<VecDeque<Renewal>>,
        calls: watch::Sender<usize>,
    }

    impl Fake {
        fn new(script: impl IntoIterator<Item = Renewal>) -> (Arc<Self>, watch::Receiver<usize>) {
            let (calls, watched) = watch::channel(0);
            let fake = Self {
                script: Mutex::new(script.into_iter().collect()),
                calls,
            };
            (Arc::new(fake), watched)
        }
    }

    #[async_trait]
    impl ContextStore for Fake {
        async fn renew(&self, lease: &Lease, lifetime: Duration) -> Result<Lease, ChatError> {
            let scripted = self.script.lock().expect("renewal script").pop_front();
            self.calls.send_modify(|calls| *calls += 1);
            match scripted {
                None | Some(Renewal::Granted) => Ok(Lease {
                    expires_at: Utc::now() + TimeDelta::from_std(lifetime).expect("a lifetime"),
                    ..lease.clone()
                }),
                Some(Renewal::Transient) => Err(ChatError::Backend(POOL_TIMED_OUT.into())),
                Some(Renewal::Lost) => Err(ChatError::LeaseLost),
                Some(Renewal::Hang) => pending().await,
            }
        }

        async fn acquire(&self, _owner: Uuid, _lifetime: Duration) -> Result<Lease, ChatError> {
            unimplemented!()
        }
        async fn assert_current(&self, _lease: &Lease) -> Result<(), ChatError> {
            unimplemented!()
        }
        async fn release(&self, _lease: &Lease) -> Result<bool, ChatError> {
            unimplemented!()
        }
        async fn begin(
            &self,
            _lease: &Lease,
            _turn_id: Uuid,
            _user_message_id: Uuid,
            _content: &str,
            _metadata: Option<Value>,
            _message: ReplayMessage,
        ) -> Result<StoredMessage, ChatError> {
            unimplemented!()
        }
        async fn append(
            &self,
            _lease: &Lease,
            _turn_id: Uuid,
            _entries: &[NewEntry],
        ) -> Result<(), ChatError> {
            unimplemented!()
        }
        async fn create_message(
            &self,
            _lease: &Lease,
            _role: &str,
            _content: &str,
            _metadata: Option<Value>,
        ) -> Result<StoredMessage, ChatError> {
            unimplemented!()
        }
        async fn delete_message(&self, _lease: &Lease, _id: Uuid) -> Result<bool, ChatError> {
            unimplemented!()
        }
        async fn consumed(&self, _lease: &Lease, _ids: &[String]) -> Result<(), ChatError> {
            unimplemented!()
        }
        async fn complete(
            &self,
            _lease: &Lease,
            _turn_id: Uuid,
            _content: &str,
            _metadata: Option<Value>,
        ) -> Result<StoredMessage, ChatError> {
            unimplemented!()
        }
        async fn publish(
            &self,
            _lease: &Lease,
            _turn_id: Uuid,
            _content: &str,
            _metadata: Option<Value>,
        ) -> Result<StoredMessage, ChatError> {
            unimplemented!()
        }
        async fn finish(
            &self,
            _lease: &Lease,
            _turn_id: Uuid,
            _content: &str,
            _metadata: Option<Value>,
            _interrupted: bool,
            _partial: Option<&ReplayMessage>,
        ) -> Result<StoredMessage, ChatError> {
            unimplemented!()
        }
        async fn interrupt(&self, _lease: &Lease, _turn_id: Uuid) -> Result<(), ChatError> {
            unimplemented!()
        }
        async fn settle(
            &self,
            _turn_id: Uuid,
            _content: Option<&str>,
            _metadata: Option<Value>,
            _partial: Option<&ReplayMessage>,
        ) -> Result<bool, ChatError> {
            unimplemented!()
        }
        async fn recover(&self, _lease: &Lease) -> Result<usize, ChatError> {
            unimplemented!()
        }
        async fn load(&self) -> Result<History, ChatError> {
            unimplemented!()
        }
        async fn checkpoint(
            &self,
            _lease: &Lease,
            _expected: Option<&Summary>,
            _proposed: &Summary,
        ) -> Result<(), ChatError> {
            unimplemented!()
        }
        async fn evidence(
            &self,
            _id: &str,
            _offset: u64,
            _limit: u64,
        ) -> Result<Evidence, ChatError> {
            unimplemented!()
        }
        async fn catalog(&self, _offset: u64, _limit: u64) -> Result<Evidence, ChatError> {
            unimplemented!()
        }
    }

    fn lease(left: Duration) -> Lease {
        Lease {
            chat_id: Uuid::new_v4(),
            owner: Uuid::new_v4(),
            fence: 1,
            expires_at: Utc::now() + TimeDelta::from_std(left).expect("a test lease fits"),
        }
    }

    fn expired(left: Duration) -> Lease {
        Lease {
            expires_at: Utc::now() - TimeDelta::from_std(left).expect("a test lease fits"),
            ..lease(left)
        }
    }

    async fn renewed(calls: &mut watch::Receiver<usize>, count: usize) -> Result<(), &'static str> {
        tokio::time::timeout(WATCHDOG, calls.wait_for(|calls| *calls >= count))
            .await
            .map(|_| ())
            .map_err(|_| "the keep-alive stopped renewing")
    }
    /// The live failure: a chat that had already saved an assistant message and
    /// tool calls lost all of them because one renewal came back with an error
    /// the keep-alive neither logged nor told apart from losing the lease.
    #[tokio::test(start_paused = true)]
    async fn a_renewal_that_fails_without_losing_the_lease_keeps_it() {
        let (store, mut calls) = Fake::new([Renewal::Transient]);
        let mut guard = keep_alive(store, lease(LIFETIME), LIFETIME).expect("a guard");

        let progress = renewed(&mut calls, 2).await;

        assert!(
            !guard.is_lost(),
            "a renewal that left the lease in the database surrendered it anyway"
        );
        progress.expect("the lease was never renewed again after one failure");
        guard.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_lease_taken_over_is_lost_without_retrying() {
        let (store, calls) = Fake::new([Renewal::Lost]);
        let mut guard = keep_alive(store, lease(LIFETIME), LIFETIME).expect("a guard");

        tokio::time::timeout(WATCHDOG, guard.lost())
            .await
            .expect("a lost lease has to stop the turn");

        assert_eq!(
            *calls.borrow(),
            1,
            "a lease someone else holds must not be asked for again"
        );
        guard.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_renewal_that_hangs_is_cut_at_what_the_lease_has_left() {
        const LEFT: Duration = Duration::from_secs(5);
        let (store, mut calls) = Fake::new([Renewal::Hang]);
        let started = Instant::now();
        let mut guard = keep_alive(store, lease(LEFT), LIFETIME).expect("a guard");

        renewed(&mut calls, 2)
            .await
            .expect("a renewal that never returns was awaited past the lease");

        assert!(
            !guard.is_lost(),
            "a renewal that timed out surrendered a lease that was still ours"
        );
        let attempt = started.elapsed() - LIFETIME / RENEWALS_PER_LIFETIME - RETRY_DELAY;
        assert!(
            attempt <= LEFT && attempt + RETRY_DELAY >= LEFT,
            "the attempt ran for {attempt:?}, not the {LEFT:?} the lease had left"
        );
        guard.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_lease_past_its_expiry_is_lost_without_another_attempt() {
        let (store, calls) = Fake::new([Renewal::Granted]);
        let mut guard = keep_alive(store, expired(LIFETIME), LIFETIME).expect("a guard");

        tokio::time::timeout(WATCHDOG, guard.lost())
            .await
            .expect("a lease nobody renewed in time has to stop the turn");

        assert_eq!(
            *calls.borrow(),
            0,
            "a lease already past its expiry must not be renewed"
        );
        guard.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_renewal_is_logged_with_its_error() {
        let (store, mut calls) = Fake::new([Renewal::Transient]);
        let lease = lease(LIFETIME);
        let chat = lease.chat_id;
        let owner = lease.owner;

        let (_guard, logged) = captured_logs(async {
            let mut guard = keep_alive(store, lease, LIFETIME).expect("a guard");
            let _ = renewed(&mut calls, 2).await;
            guard.stop().await;
            guard
        })
        .await;

        assert!(
            logged.contains(POOL_TIMED_OUT),
            "the error the renewal failed with is missing from {logged:?}"
        );
        assert!(
            logged.contains(&chat.to_string()) && logged.contains(&owner.to_string()),
            "the chat and owner are missing from {logged:?}"
        );
    }
}
