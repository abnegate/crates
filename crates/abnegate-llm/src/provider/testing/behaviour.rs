/// What a [`StubProvider`](super::StubProvider) does when asked.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Behaviour {
    Answer(String),
    /// Fails in a way a chain is expected to move past.
    Fail(String),
    /// Fails in a way a chain must not move past.
    Reject(String),
}
