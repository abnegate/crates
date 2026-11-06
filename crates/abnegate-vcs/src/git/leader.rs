/// The first process of a [`Group`](crate::git::group::Group), whose
/// identifier is the group's. A leader that has exited but goes unreaped
/// still holds that identifier, so no other group can be handed it while
/// this one is being killed.
pub(crate) trait Leader: Send + 'static {
    /// Collect the leader's exit without waiting for it, and say whether it
    /// is reaped, or can no longer be.
    fn reaped(&mut self) -> bool;
}

impl Leader for std::process::Child {
    fn reaped(&mut self) -> bool {
        !matches!(self.try_wait(), Ok(None))
    }
}

impl Leader for tokio::process::Child {
    fn reaped(&mut self) -> bool {
        !matches!(self.try_wait(), Ok(None))
    }
}
