use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

use super::Error;
use super::Evidence;
use super::History;
use super::Lease;
use super::NewEntry;
use super::ReplayMessage;
use super::StoredMessage;
use super::Summary;

/// The durable conversation behind one chat.
///
/// Writes are gated on a [`Lease`], so a chat can only ever have one live
/// response: whoever holds the lease owns the turn, and a stale holder is told
/// [`Error::LeaseLost`] rather than being allowed to append. The one exception
/// is [`ContextStore::settle`], which closes a turn whose lease is already
/// gone: it can reach nothing but the one turn it names.
#[async_trait]
pub trait ContextStore: Send + Sync {
    /// Take the right to respond in this chat, or fail with [`Error::Busy`].
    async fn acquire(&self, owner: Uuid, lifetime: Duration) -> Result<Lease, Error>;

    /// Extend a lease that is still ours.
    async fn renew(&self, lease: &Lease, lifetime: Duration) -> Result<Lease, Error>;

    /// Fail unless this lease is still the current one.
    async fn assert_current(&self, lease: &Lease) -> Result<(), Error>;

    /// Give the lease up. False when it had already been taken over.
    async fn release(&self, lease: &Lease) -> Result<bool, Error>;

    /// Record the user message that opens a turn.
    async fn begin(
        &self,
        lease: &Lease,
        turn_id: Uuid,
        user_message_id: Uuid,
        content: &str,
        metadata: Option<Value>,
        message: ReplayMessage,
    ) -> Result<StoredMessage, Error>;

    /// Append entries produced while the turn runs.
    async fn append(&self, lease: &Lease, turn_id: Uuid, entries: &[NewEntry])
    -> Result<(), Error>;

    async fn create_message(
        &self,
        lease: &Lease,
        role: &str,
        content: &str,
        metadata: Option<Value>,
    ) -> Result<StoredMessage, Error>;

    async fn delete_message(&self, lease: &Lease, id: Uuid) -> Result<bool, Error>;

    /// Mark evidence as folded into the visible history.
    async fn consumed(&self, lease: &Lease, ids: &[String]) -> Result<(), Error>;

    async fn complete(
        &self,
        lease: &Lease,
        turn_id: Uuid,
        content: &str,
        metadata: Option<Value>,
    ) -> Result<StoredMessage, Error>;

    async fn publish(
        &self,
        lease: &Lease,
        turn_id: Uuid,
        content: &str,
        metadata: Option<Value>,
    ) -> Result<StoredMessage, Error>;

    /// Close a turn, durably, whether it ran to the end or was interrupted.
    async fn finish(
        &self,
        lease: &Lease,
        turn_id: Uuid,
        content: &str,
        metadata: Option<Value>,
        interrupted: bool,
        partial: Option<&ReplayMessage>,
    ) -> Result<StoredMessage, Error>;

    async fn interrupt(&self, lease: &Lease, turn_id: Uuid) -> Result<(), Error>;

    /// Close a turn whose lease is already gone, so a lost lease cannot leave a
    /// row running for ever. A turn id belongs to one generation, so no other
    /// writer owns that row. A turn a successor's recovery has already closed
    /// still takes the prose this generation streamed, once. False when there
    /// was nothing left for this call to do.
    async fn settle(
        &self,
        turn_id: Uuid,
        content: Option<&str>,
        metadata: Option<Value>,
        partial: Option<&ReplayMessage>,
    ) -> Result<bool, Error>;

    /// Settle turns a previous process left open. Returns how many.
    async fn recover(&self, lease: &Lease) -> Result<usize, Error>;

    async fn load(&self) -> Result<History, Error>;

    /// Replace the summary, refusing when someone else moved it first.
    async fn checkpoint(
        &self,
        lease: &Lease,
        expected: Option<&Summary>,
        proposed: &Summary,
    ) -> Result<(), Error>;

    async fn evidence(&self, id: &str, offset: u64, limit: u64) -> Result<Evidence, Error>;

    async fn catalog(&self, offset: u64, limit: u64) -> Result<Evidence, Error>;
}
