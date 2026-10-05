use std::collections::BTreeMap;
use std::collections::BTreeSet;

use super::Marker;
use super::ProcessGroup;

/// What a [`GroupRegistry`](super::GroupRegistry) holds behind its lock: the
/// registered groups by id, the tracked markers, and how far the host has
/// got in stopping.
#[derive(Debug, Default)]
pub(super) struct State {
    pub(super) groups: BTreeMap<i32, ProcessGroup>,
    pub(super) markers: BTreeSet<Marker>,
    /// A [`shutdown`](super::GroupRegistry::shutdown) has begun.
    pub(super) stopping: bool,
    pub(super) killed: bool,
}

impl State {
    /// Unregister `group` if the handle registered under its id is a clone of
    /// it, so a run never unregisters a later group that reused its id.
    pub(super) fn remove(&mut self, group: &ProcessGroup) -> bool {
        let registered = self
            .groups
            .get(&group.pgid())
            .is_some_and(|registered| registered.is_clone_of(group));
        if registered {
            self.groups.remove(&group.pgid());
        }
        registered
    }

    pub(super) fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.markers.is_empty()
    }
}
