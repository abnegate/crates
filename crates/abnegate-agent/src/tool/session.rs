use uuid::Uuid;

/// Which conversation or task run a tool call belongs to.
///
/// Background jobs and waits are keyed on it: a job started by one session is
/// unreadable from another, and a detached context can start neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Session {
    /// Belongs to no conversation or task, so it can start no background job
    /// and wait on none.
    Detached,
    /// A chat conversation, by its id.
    Chat(Uuid),
    /// A task run, by its id.
    Task(Uuid),
}
