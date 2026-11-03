use std::io;
use std::process::ExitStatus;
use std::time::Duration;

use tokio::process::Child;
use tokio::process::Command;

use super::GroupRegistry;
use super::Marker;
use super::ProcessGroup;
use crate::executor::marker::straggler::VARIABLE;

/// How long the processes a run left behind get to exit on SIGTERM before
/// [`Run::finish`] kills them.
pub(super) const TERMINATE_GRACE: Duration = Duration::from_secs(2);

/// One run, registered the way a host registers each of its runs: its marker
/// tracked before it spawns, which refuses the run once the registry is
/// killed, and its group inserted once it has. Its group is
/// killed and removed once its leader exits ([`wait`](Self::wait)), on
/// [`finish`](Self::finish), or on drop; finishing or dropping it also sweeps
/// its marker and stops tracking it.
pub(super) struct Run<'a> {
    group: Option<ProcessGroup>,
    marker: Option<Marker>,
    registry: &'a GroupRegistry,
}

impl<'a> Run<'a> {
    pub(super) fn spawn(
        command: &mut Command,
        registry: &'a GroupRegistry,
    ) -> io::Result<(Child, Self)> {
        let marker = Marker::new(VARIABLE).expect("a valid variable");
        marker.apply(command);
        if !registry.track(marker.clone()) {
            return Err(io::Error::other("the registry was killed"));
        }
        let child = command
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .inspect_err(|_| {
                registry.release(&marker);
            })?;
        let group = child
            .id()
            .map(|pid| ProcessGroup::try_from(pid).expect("a child leading its own group"));
        if let Some(group) = &group {
            registry
                .insert(group.clone())
                .expect("the group takes the signal");
        }
        let run = Self {
            group,
            marker: Some(marker),
            registry,
        };
        Ok((child, run))
    }

    /// Wait for the leader to exit, kill what it left in its group, then
    /// remove and release the group and reap the leader.
    pub(super) async fn wait(&mut self, child: &mut Child) -> io::Result<ExitStatus> {
        if let Some(group) = self.group.clone() {
            group.leader_exited().await.expect("the leader is watched");
            group
                .kill_until_gone()
                .await
                .expect("the group takes SIGKILL");
            self.registry.remove(&group);
            group.release();
            self.group = None;
        }
        child.wait().await
    }

    /// Kill the group, then give the marked processes that left it
    /// [`TERMINATE_GRACE`] to exit on SIGTERM before they are killed.
    pub(super) async fn finish(&mut self) {
        self.kill_group();
        if let Some(marker) = self.marker.clone() {
            let swept = marker.clone();
            tokio::task::spawn_blocking(move || swept.terminate(TERMINATE_GRACE))
                .await
                .expect("the sweep runs")
                .expect("the sweep succeeds");
            self.registry.release(&marker);
            self.marker = None;
        }
    }

    fn kill_group(&mut self) {
        if let Some(group) = self.group.take() {
            let _ = self.registry.kill(&group);
        }
    }
}

impl Drop for Run<'_> {
    fn drop(&mut self) {
        self.kill_group();
        if let Some(marker) = self.marker.take() {
            let _ = marker.kill();
            self.registry.release(&marker);
        }
    }
}
