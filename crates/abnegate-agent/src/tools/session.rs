use uuid::Uuid;

/// Which conversation or task run a tool call belongs to.
///
/// Background jobs and waits are keyed on it: a job started by one session is
/// unreadable from another, and a detached context can start neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Session {
    Detached,
    Chat(Uuid),
    Task(Uuid),
}
