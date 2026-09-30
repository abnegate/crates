use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

use super::ChatError;
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
/// [`ChatError::LeaseLost`] rather than being allowed to append. The one exception
/// is [`ContextStore::settle`], which closes a turn whose lease is already
/// gone: it can reach nothing but the one turn it names.
#[async_trait]
pub trait ContextStore: Send + Sync {
    /// Take the right to respond in this chat, or fail with [`ChatError::Busy`].
    async fn acquire(&self, owner: Uuid, lifetime: Duration) -> Result<Lease, ChatError>;

    /// Extend a lease that is still ours.
    async fn renew(&self, lease: &Lease, lifetime: Duration) -> Result<Lease, ChatError>;

    /// Fail unless this lease is still the current one.
    async fn assert_current(&self, lease: &Lease) -> Result<(), ChatError>;

    /// Give the lease up. False when it had already been taken over.
    async fn release(&self, lease: &Lease) -> Result<bool, ChatError>;

    /// Record the user message that opens a turn.
    async fn begin(
        &self,
        lease: &Lease,
        turn_id: Uuid,
        user_message_id: Uuid,
        content: &str,
        metadata: Option<Value>,
        message: ReplayMessage,
    ) -> Result<StoredMessage, ChatError>;

    /// Append entries produced while the turn runs.
    async fn append(
        &self,
        lease: &Lease,
        turn_id: Uuid,
        entries: &[NewEntry],
    ) -> Result<(), ChatError>;

    /// Store a visible `user`, `assistant` or `system` message outside any
    /// turn. A user message may claim the chat's automatic title.
    async fn create_message(
        &self,
        lease: &Lease,
        role: &str,
        content: &str,
        metadata: Option<Value>,
    ) -> Result<StoredMessage, ChatError>;

    /// Delete message `id`, and the evidence it owns, invalidating the
    /// checkpoint. False when there was no such message.
    async fn delete_message(&self, lease: &Lease, id: Uuid) -> Result<bool, ChatError>;

    /// Mark evidence as folded into the visible history.
    async fn consumed(&self, lease: &Lease, ids: &[String]) -> Result<(), ChatError>;

    /// Close turn `turn_id` with its final answer. Fails with
    /// [`ChatError::Integrity`] while any of its tool calls still awaits an
    /// outcome.
    async fn complete(
        &self,
        lease: &Lease,
        turn_id: Uuid,
        content: &str,
        metadata: Option<Value>,
    ) -> Result<StoredMessage, ChatError>;

    /// Write the running turn's visible answer so far, so a reader that
    /// reloads sees streamed text before the turn ends. The turn stays open.
    async fn publish(
        &self,
        lease: &Lease,
        turn_id: Uuid,
        content: &str,
        metadata: Option<Value>,
    ) -> Result<StoredMessage, ChatError>;

    /// Close a turn, durably, whether it ran to the end or was interrupted.
    async fn finish(
        &self,
        lease: &Lease,
        turn_id: Uuid,
        content: &str,
        metadata: Option<Value>,
        interrupted: bool,
        partial: Option<&ReplayMessage>,
    ) -> Result<StoredMessage, ChatError>;

    /// Mark turn `turn_id` interrupted without an answer, recording every tool
    /// call still waiting as having an unknown outcome.
    async fn interrupt(&self, lease: &Lease, turn_id: Uuid) -> Result<(), ChatError>;

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
    ) -> Result<bool, ChatError>;

    /// Settle turns a previous process left open. Returns how many.
    async fn recover(&self, lease: &Lease) -> Result<usize, ChatError>;

    /// Read the whole conversation, with its checkpoint validated against
    /// it. Needs no lease: it writes nothing.
    async fn load(&self) -> Result<History, ChatError>;

    /// Replace the summary, refusing when someone else moved it first.
    async fn checkpoint(
        &self,
        lease: &Lease,
        expected: Option<&Summary>,
        proposed: &Summary,
    ) -> Result<(), ChatError>;

    /// Up to `limit` characters of the tool output recorded as `id`, from
    /// `offset`. [`ChatError::NotFound`] when no such output belongs to this
    /// chat. An `id` returned by [`catalog`](Self::catalog) continues that
    /// catalog's snapshot.
    async fn evidence(&self, id: &str, offset: u64, limit: u64) -> Result<Evidence, ChatError>;

    /// A page of the index of every piece of evidence in the chat, taken as a
    /// snapshot so reading it cannot make it grow. Its [`Evidence::id`] reads
    /// the rest through [`evidence`](Self::evidence).
    async fn catalog(&self, offset: u64, limit: u64) -> Result<Evidence, ChatError>;
}
