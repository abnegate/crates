//! Cleanup for a run's process group and marker, on every path a run can end
//! on.

use std::sync::Arc;
use std::time::Duration;

use abnegate_exec::GroupRegistry;
use abnegate_exec::Marker;
use abnegate_exec::MarkerError;
use abnegate_exec::executor::GRACE_PERIOD;
use abnegate_exec::executor::ProcessGroup;

/// The name of the thread a dropped run's marked processes are killed from.
const SWEEPER: &str = "agent-marker-sweep";

/// How long a run waits on the sweep of its marked processes: the grace it
/// gives them to exit, and as long again for the sweep's own reads.
const SWEEP_LIMIT: Duration = GRACE_PERIOD.saturating_mul(2);

/// Stops what a run's agent left behind, and keeps the run's entries in its
/// [`GroupRegistry`] in step with it.
///
/// `kill_on_drop` reaches only the direct child, so a cancelled run would
/// otherwise orphan everything the agent forked. A run holds this alongside
/// the child in a [`Spawned`](crate::spawned::Spawned), which drops it first,
/// so a cancelled run kills the group while its leader is still unreaped and
/// the group's id cannot belong to anyone else, and kills every process
/// carrying its marker. The drop repeats the kill, which catches a child
/// forked as the first one lands, on its own thread while the group still
/// answers, for at most a tenth of a second, then releases the group, so no
/// repeat lands once the child, dropped next, lets the leader be reaped. The
/// marker's sweep follows from a thread of its own.
///
/// A run that ends by itself [cleans up](Reaper::clean) while its leader is
/// still unreaped too, and [releases](Reaper::release) the group only just
/// before it reaps the leader, so no kill of the group ever lands after the
/// leader is reaped.
#[derive(Debug)]
pub(crate) struct Reaper {
    group: Option<ProcessGroup>,
    marker: Option<Marker>,
    registry: Option<Arc<GroupRegistry>>,
    /// The run spawned its agent, which may have started marked processes
    /// whether or not it leads a group the run could take charge of.
    spawned: bool,
    cleaned: bool,
}

impl Reaper {
    /// A reaper for a run about to spawn its agent, with `marker` tracked in
    /// `registry` from now on. `None` once the registry has been killed, after
    /// which its host starts no run.
    pub(crate) fn admit(
        registry: Option<Arc<GroupRegistry>>,
        marker: Option<Marker>,
    ) -> Option<Self> {
        let admitted = match (&registry, &marker) {
            (None, _) => true,
            (Some(registry), Some(marker)) => registry.track(marker.clone()),
            (Some(registry), None) => !registry.is_killed(),
        };
        if !admitted {
            return None;
        }
        Some(Self {
            group: None,
            marker,
            registry,
            spawned: false,
            cleaned: false,
        })
    }

    /// The marker every process of the run carries.
    pub(crate) fn marker(&self) -> Option<&Marker> {
        self.marker.as_ref()
    }

    /// Take charge of the agent the run spawned, and of the group the agent
    /// `pid` leads, which is registered.
    pub(crate) fn lead(&mut self, pid: Option<u32>) {
        self.spawned = true;
        self.group = pid.and_then(|pid| ProcessGroup::try_from(pid).ok());
        if let (Some(registry), Some(group)) = (&self.registry, &self.group)
            && let Err(error) = registry.insert(group.clone())
        {
            tracing::debug!(%error, "the registry could not signal the agent's group as it registered it");
        }
    }

    pub(crate) fn group(&self) -> Option<&ProcessGroup> {
        self.group.as_ref()
    }

    /// Kill whatever is left in the agent's group, then sweep every process
    /// carrying the run's marker, giving each [`GRACE_PERIOD`] to exit on
    /// SIGTERM, waiting on the sweep at most [`SWEEP_LIMIT`]. Only the first
    /// call does anything. Call it only while the group's leader is unreaped.
    pub(crate) async fn clean(&mut self) {
        if std::mem::replace(&mut self.cleaned, true) {
            return;
        }
        if let Some(group) = &self.group
            && let Err(error) = group.kill_until_gone().await
        {
            tracing::debug!(%error, "the agent's process group refused the kill");
        }
        let Some(marker) = self.marker.take() else {
            return;
        };
        let registry = self.registry.clone();
        sweep_within(SWEEP_LIMIT, move || {
            let swept = marker.terminate(GRACE_PERIOD);
            if let Some(registry) = registry {
                registry.release(&marker);
            }
            swept
        })
        .await;
    }

    /// Stop signalling the group and unregister it, just before its leader
    /// is reaped, and stop tracking a marker no sweep took.
    pub(crate) fn release(&mut self) {
        if let Some(group) = self.group.take() {
            if let Some(registry) = &self.registry {
                registry.remove(&group);
            }
            group.release();
        }
        self.forget();
    }

    fn forget(&mut self) {
        if let (Some(registry), Some(marker)) = (&self.registry, self.marker.take()) {
            registry.release(&marker);
        }
    }
}

impl Drop for Reaper {
    fn drop(&mut self) {
        if let Some(group) = self.group.take() {
            if let Some(registry) = &self.registry {
                registry.remove(&group);
            }
            if let Err(error) = group.kill_blocking() {
                tracing::debug!(%error, "the agent's process group refused the kill");
            }
            group.release();
        }
        if self.spawned
            && let Some(marker) = self.marker.take()
        {
            kill_in_background(marker, self.registry.clone());
        }
        self.forget();
    }
}

/// Run `sweep` on the blocking pool, waiting for it at most `limit`.
///
/// A sweep reads every process's environment, and a read of one stuck in the
/// kernel blocks until that process answers, which may be never. Past the
/// limit the run goes on, and the sweep is left to finish on the blocking
/// pool, releasing its marker when it does.
async fn sweep_within(
    limit: Duration,
    sweep: impl FnOnce() -> Result<Vec<u32>, MarkerError> + Send + 'static,
) {
    match tokio::time::timeout(limit, tokio::task::spawn_blocking(sweep)).await {
        Ok(Ok(Ok(_))) => {}
        Ok(Ok(Err(error))) => {
            tracing::warn!(%error, "could not stop every process the agent left outside its group");
        }
        Ok(Err(error)) => {
            tracing::warn!(%error, "the sweep for processes the agent left outside its group did not finish");
        }
        Err(_) => {
            tracing::warn!(
                limit_seconds = limit.as_secs_f64(),
                "the sweep for processes the agent left outside its group outlasted its limit; leaving it to finish in the background"
            );
        }
    }
}

/// [`kill_marked`] on a thread of its own, or on the caller's when no thread
/// can be started.
fn kill_in_background(marker: Marker, registry: Option<Arc<GroupRegistry>>) {
    let swept = marker.clone();
    let held = registry.clone();
    let started = std::thread::Builder::new()
        .name(SWEEPER.to_string())
        .spawn(move || kill_marked(&swept, held.as_deref()));
    if started.is_err() {
        kill_marked(&marker, registry.as_deref());
    }
}

fn kill_marked(marker: &Marker, registry: Option<&GroupRegistry>) {
    if let Err(error) = marker.kill() {
        tracing::warn!(%error, "could not kill every process the agent left outside its group");
    }
    if let Some(registry) = registry {
        registry.release(marker);
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::process::Stdio;
    use std::sync::Arc;
    use std::time::Duration;
    use std::time::Instant;

    use abnegate_exec::GroupRegistry;
    use abnegate_exec::Marker;
    use tempfile::TempDir;

    use super::Reaper;
    use super::sweep_within;
    use crate::test_support::MARKER;
    use crate::test_support::PATIENCE;
    use crate::test_support::Straggler;
    use crate::test_support::running;
    use crate::test_support::stop_if_running;

    fn sleeper() -> std::process::Child {
        Command::new("sleep")
            .arg("120")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("sleep to start")
    }

    fn exits_within(child: &mut std::process::Child, limit: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            if child.try_wait().expect("a status").is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn leading(child: &std::process::Child) -> Reaper {
        let mut reaper = Reaper::admit(None, None).expect("a reaper without a registry");
        reaper.lead(Some(child.id()));
        reaper
    }

    fn registry() -> Arc<GroupRegistry> {
        Arc::new(GroupRegistry::new())
    }

    #[test]
    fn dropping_a_reaper_kills_the_group() {
        let mut child = sleeper();
        drop(leading(&child));

        assert!(
            exits_within(&mut child, PATIENCE),
            "dropping the reaper left its group running"
        );
    }

    fn eventually(condition: impl Fn() -> bool) -> bool {
        let start = Instant::now();
        while !condition() {
            if start.elapsed() > PATIENCE {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

    /// A cancelled run drops its reaper whatever the agent is doing, forking
    /// included. Each of the shell's sleeps outlives [`PATIENCE`], so one the
    /// kill missed is still running when the test gives up on the group.
    #[test]
    fn dropping_a_reaper_reaches_a_child_forked_as_it_fires() {
        let mut shell = Command::new("sh")
            .args(["-c", "while :; do sleep 300 & done"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("the shell starts");
        let leader = shell.id();
        assert!(
            eventually(|| running(leader).len() > 2),
            "the shell never started forking"
        );

        drop(leading(&shell));
        shell.wait().expect("the shell is reaped");

        let gone = eventually(|| running(leader).is_empty());
        let survivors = running(leader);
        for pid in &survivors {
            let _ = Command::new("kill").args(["-9", pid]).status();
        }
        assert!(gone, "the reaper's kill missed {survivors:?}");
    }

    /// An agent that spawned but leads no group the run could take charge
    /// of can still have started processes that carry the run's marker.
    #[test]
    fn dropping_a_reaper_that_spawned_without_a_group_kills_its_marked_processes() {
        let directory = TempDir::new().expect("a temporary directory");
        let marker = Marker::new(MARKER).unwrap();
        let mut reaper =
            Reaper::admit(None, Some(marker.clone())).expect("a reaper without a registry");
        let started = Command::new("sh")
            .arg("-c")
            .arg(Straggler::Escapes.script(directory.path(), false))
            .env(marker.variable(), marker.value())
            .status()
            .expect("the agent runs");
        assert!(started.success());
        reaper.lead(None);
        let escaped = Straggler::pid(directory.path());

        drop(reaper);

        let stopped = crate::test_support::exits_within(escaped);
        stop_if_running(escaped);
        assert!(
            stopped,
            "process {escaped} carrying the marker outlived the dropped run"
        );
    }

    /// A released group is no longer the handle's to watch, so its leader's
    /// exit resolves without a status, while one still held reports the
    /// SIGKILL that ended it.
    #[tokio::test]
    async fn a_dropped_reaper_releases_its_group_once_its_kill_is_done() {
        let mut child = sleeper();
        let reaper = leading(&child);
        let group = reaper.group().cloned().expect("the agent's group");

        drop(reaper);

        let exited = tokio::time::timeout(PATIENCE, group.leader_exited())
            .await
            .expect("the leader's exit resolves")
            .expect("the leader is watched");
        let _ = child.wait();
        assert!(
            exited.is_none(),
            "the dropped reaper left its group unreleased, to be signalled after its leader is reaped: {exited:?}"
        );
    }

    #[test]
    fn a_released_reaper_leaves_the_group_running() {
        let mut child = sleeper();
        let mut reaper = leading(&child);
        assert!(reaper.group().is_some());
        reaper.release();
        drop(reaper);

        assert!(!exits_within(&mut child, Duration::from_millis(200)));
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn a_registry_that_was_killed_admits_no_run() {
        let registry = registry();
        registry.kill_all().expect("an empty registry is killed");

        assert!(Reaper::admit(Some(Arc::clone(&registry)), None).is_none());
        assert!(
            Reaper::admit(
                Some(Arc::clone(&registry)),
                Some(Marker::new(MARKER).unwrap())
            )
            .is_none()
        );
        assert!(registry.is_empty(), "a refused run left its marker tracked");
    }

    #[test]
    fn a_run_is_registered_until_it_releases_its_group() {
        let registry = registry();
        let mut child = sleeper();
        let mut reaper = Reaper::admit(
            Some(Arc::clone(&registry)),
            Some(Marker::new(MARKER).unwrap()),
        )
        .expect("a live registry admits the run");
        assert!(
            !registry.is_empty(),
            "the marker is tracked before the spawn"
        );
        reaper.lead(Some(child.id()));

        reaper.release();

        assert!(registry.is_empty(), "the released run is still registered");
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn a_run_that_never_spawned_leaves_nothing_tracked() {
        let registry = registry();
        let reaper = Reaper::admit(
            Some(Arc::clone(&registry)),
            Some(Marker::new(MARKER).unwrap()),
        )
        .expect("a live registry admits the run");

        drop(reaper);

        assert!(registry.is_empty());
    }

    #[test]
    fn dropping_a_registered_reaper_kills_its_group_and_unregisters_it() {
        let registry = registry();
        let mut child = sleeper();
        let mut reaper = Reaper::admit(
            Some(Arc::clone(&registry)),
            Some(Marker::new(MARKER).unwrap()),
        )
        .expect("a live registry admits the run");
        reaper.lead(Some(child.id()));

        drop(reaper);

        assert!(
            exits_within(&mut child, PATIENCE),
            "dropping the reaper left its group running"
        );
        assert!(
            eventually(|| registry.is_empty()),
            "the dropped run is still registered"
        );
    }

    /// A read of `/proc/<pid>/environ` blocks while that process is stuck
    /// in the kernel, which this sweep stands in for by never returning on
    /// its own.
    #[tokio::test]
    async fn a_sweep_that_never_returns_holds_the_run_up_only_until_its_limit() {
        let (finish, finished) = std::sync::mpsc::channel::<()>();

        let waited = tokio::time::timeout(
            PATIENCE,
            sweep_within(Duration::from_millis(100), move || {
                let _ = finished.recv();
                Ok(Vec::new())
            }),
        )
        .await;

        let _ = finish.send(());
        assert!(
            waited.is_ok(),
            "the run waited on a sweep stuck in the kernel past its limit"
        );
    }

    #[tokio::test]
    async fn cleaning_up_kills_the_group_and_releases_the_marker() {
        let registry = registry();
        let mut child = sleeper();
        let mut reaper = Reaper::admit(
            Some(Arc::clone(&registry)),
            Some(Marker::new(MARKER).unwrap()),
        )
        .expect("a live registry admits the run");
        reaper.lead(Some(child.id()));

        reaper.clean().await;

        assert!(
            exits_within(&mut child, PATIENCE),
            "cleaning up left the group running"
        );
        assert!(reaper.marker().is_none(), "the marker was not swept");
        reaper.release();
        assert!(registry.is_empty());
    }
}
