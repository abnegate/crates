//! A spawned agent held together with what cleans up after it.

use tokio::process::Child;

use crate::reaper::Reaper;

/// A spawned agent and the [`Reaper`] that stops what it left behind, which
/// drops first however the run holding them ends.
///
/// `kill_on_drop` kills only the agent, and the runtime reaps it soon after,
/// from when its group's id may belong to another group. Declared ahead of
/// the child, the reaper kills and releases the group while the child still
/// holds the agent unreaped, on every path from the spawn on, a run
/// cancelled before it is built included.
pub(crate) struct Spawned {
    pub(crate) reaper: Reaper,
    pub(crate) child: Child,
}

impl Spawned {
    /// Hold `child` with `reaper`, which takes charge of the child's group.
    pub(crate) fn new(mut reaper: Reaper, child: Child) -> Self {
        reaper.lead(child.id());
        Self { reaper, child }
    }
}
