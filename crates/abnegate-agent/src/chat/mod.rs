//! Chat history, context capacity, and the storage a chat session needs.
//!
//! [`ContextStore`] is the boundary: a session leases the right to respond in
//! one chat, writes the turn through that lease, and reads history back,
//! without naming a database. Writes are gated on a [`Lease`], so a chat can
//! only ever have one live response, and [`keep_alive`] renews that lease on
//! its own schedule so a slow tool cannot let it lapse mid-turn.
//!
//! [`History`] is what the store loads, kept in [`ReplayMessage`]s that are
//! independent of the provider's wire format; [`validate`] checks a
//! [`Summary`] against it before the summary may stand in for any evidence.
//! [`Resolver`] finds the effective context [`Capacity`] of the deployment a
//! model alias routes to.

mod capacity;
mod entry;
mod error;
mod evidence;
mod guard;
mod history;
mod lease;
mod new_entry;
mod parameters;
mod replay;
mod resolver;
mod route;
mod routes;
mod store;
mod stored;
mod summary;

pub use capacity::Capacity;
pub use entry::Entry;
pub use error::ChatError;
pub use evidence::Evidence;
pub use guard::Guard;
pub use guard::keep_alive;
pub use history::History;
pub use history::fingerprint;
pub use history::validate;
pub use lease::Lease;
pub use new_entry::NewEntry;
pub use replay::ReplayMessage;
pub use replay::VERSION;
pub use resolver::DEFAULT_CONTEXT;
pub use resolver::Resolver;
pub use store::ContextStore;
pub use stored::StoredMessage;
pub use summary::Summary;

pub use crate::context::ContextSource as Source;
