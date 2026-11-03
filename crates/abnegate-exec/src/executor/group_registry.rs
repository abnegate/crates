use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinError;

use crate::error::ExecutorError;
use crate::error::GroupRegistryError;
use crate::error::MarkerError;

use super::marker::Marker;
use super::process_group::ProcessGroup;

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod run;
mod state;
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests;

use state::State;

/// A tracked marker and what its sweep stopped.
type Sweep = (Marker, Result<Vec<u32>, MarkerError>);

static GLOBAL: LazyLock<Arc<GroupRegistry>> = LazyLock::new(|| Arc::new(GroupRegistry::new()));

/// The process groups of a host's runs that are still going, and the
/// [`Marker`]s of runs whose processes may have left their groups, so the
/// host can pass a Ctrl-C on to every run and stop whatever is left when it
/// shuts down.
///
/// A run that leads a process group of its own is out of reach of the
/// signals a terminal sends its host. The host tracks each run's marker
/// before it spawns the run and registers the run's group once it has, so a
/// shutdown can [interrupt](GroupRegistry::interrupt_all) every group, wait
/// for the runs to [empty](GroupRegistry::emptied) the registry, and then
/// [kill](GroupRegistry::kill_all) every group and sweep every marker still
/// tracked.
///
/// [`interrupt_all`](GroupRegistry::interrupt_all) is a Ctrl-C passed on: it
/// reaches every group registered when it is called, every time it is
/// called, and leaves the registry as it was, so a run started after it goes
/// on undisturbed. A [`shutdown`](GroupRegistry::shutdown) is sticky: once it
/// has begun, a group registered is interrupted as it registers, and one
/// registered after [`kill_all`](GroupRegistry::kill_all) is killed instead of
/// registered, so a run that spawns as its host stops is stopped too. Once
/// killed, the registry tracks no more markers, which tells a host to start no
/// more runs.
///
/// A registered group is signalled through its own [`ProcessGroup`] handle, so
/// once its run [releases](ProcessGroup::release) the group before reaping its
/// leader, nothing here signals the group's id again. macOS refuses to signal
/// a group whose members are all unreaped zombies; such a group has nothing
/// left to stop, so that refusal is not reported as a failure.
///
/// A run must [remove](GroupRegistry::remove) or [kill](GroupRegistry::kill)
/// its group and [release](GroupRegistry::release) its marker on every path it
/// can end on, a failed spawn and a drop included, or
/// [`emptied`](GroupRegistry::emptied) waits for it until a
/// [`kill_all`](GroupRegistry::kill_all).
///
/// ```no_run
/// use std::time::Duration;
///
/// use abnegate_exec::GroupRegistry;
/// use abnegate_exec::Marker;
/// use abnegate_exec::executor::ProcessGroup;
/// use tokio::process::Command;
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let registry = GroupRegistry::global();
/// let marker = Marker::new("MY_APPLICATION_RUN")?;
/// if !registry.track(marker.clone()) {
///     return Err("the host is stopping".into());
/// }
/// let mut command = Command::new("my-agent");
/// command.process_group(0).kill_on_drop(true);
/// marker.apply(&mut command);
/// let mut child = command.spawn().inspect_err(|_| {
///     registry.release(&marker);
/// })?;
/// let group = ProcessGroup::try_from(child.id().ok_or("the agent was reaped")?)?;
/// registry.insert(group.clone())?;
///
/// group.leader_exited().await?;
/// group.kill_until_gone().await?;
/// registry.remove(&group);
/// group.release();
/// child.wait().await?;
/// let swept = marker.clone();
/// tokio::task::spawn_blocking(move || swept.terminate(Duration::from_secs(5))).await??;
/// registry.release(&marker);
///
/// // Elsewhere, once the host is told to stop:
/// registry.shutdown(Duration::from_secs(5)).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct GroupRegistry {
    state: Mutex<State>,
    empty: watch::Sender<bool>,
}

impl Default for GroupRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl GroupRegistry {
    /// An empty registry whose host is not stopping.
    ///
    /// A host usually keeps one registry for all of its runs, such as
    /// [`global`](Self::global); one of its own suits a host that stops some
    /// runs apart from the rest.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            empty: watch::Sender::new(true),
        }
    }

    /// The registry of the whole process, for a host whose signal handler
    /// cannot be handed one. Every call shares the same registry, and the
    /// handle can be kept, or handed to each run, for as long as it is
    /// needed.
    pub fn global() -> Arc<Self> {
        Arc::clone(&GLOBAL)
    }

    /// Register `group` until its run [removes](Self::remove) or
    /// [kills](Self::kill) it. Once a [shutdown](Self::shutdown) has begun
    /// it is interrupted as it registers, and once the registry has been
    /// [killed](Self::kill_all) it is killed instead of registered.
    ///
    /// A handle built apart for the id of a group already registered
    /// replaces it, and the earlier handle no longer removes or kills
    /// anything. An id is reused only once the leader holding it has been
    /// reaped, by which time its run has released and removed its group.
    ///
    /// # Errors
    ///
    /// [`ExecutorError::ProcessGroupFailed`] when the group refuses the
    /// interrupt or the kill. It is registered all the same unless it was to
    /// be killed.
    pub fn insert(&self, group: ProcessGroup) -> Result<(), ExecutorError> {
        let (killed, stopping) = {
            let mut state = self.state();
            if !state.killed {
                state.groups.insert(group.pgid(), group.clone());
                self.publish(&state);
            }
            (state.killed, state.stopping)
        };
        if killed {
            group.kill()
        } else if stopping {
            group.interrupt()
        } else {
            Ok(())
        }
    }

    /// Unregister `group` without signalling it, for a run that stopped its
    /// group itself. Returns whether it was registered: not once
    /// [`kill_all`](Self::kill_all) has taken it, nor once a handle built
    /// apart for the same id has replaced it.
    pub fn remove(&self, group: &ProcessGroup) -> bool {
        let mut state = self.state();
        let removed = state.remove(group);
        self.publish(&state);
        removed
    }

    /// Unregister `group` and kill it, as [`ProcessGroup::kill`] does. A group
    /// that is not registered is left alone, so one that
    /// [`kill_all`](Self::kill_all) already killed is not signalled again.
    /// Returns whether it was registered, and so killed.
    ///
    /// # Errors
    ///
    /// [`ExecutorError::ProcessGroupFailed`] when the group refuses the kill.
    /// It is unregistered all the same.
    pub fn kill(&self, group: &ProcessGroup) -> Result<bool, ExecutorError> {
        let removed = self.remove(group);
        if removed {
            group.kill()?;
        }
        Ok(removed)
    }

    /// Track `marker` until its run [releases](Self::release) it, once the
    /// run has swept the processes that carry it, so that
    /// [`kill_all`](Self::kill_all) reaches them meanwhile. Track it before
    /// spawning the command it marks.
    ///
    /// Returns whether it is tracked: not once the registry has been
    /// [killed](Self::kill_all), when the host should not start the run.
    pub fn track(&self, marker: Marker) -> bool {
        let mut state = self.state();
        if state.killed {
            return false;
        }
        state.markers.insert(marker);
        self.publish(&state);
        true
    }

    /// Stop tracking `marker`. Returns whether it was tracked.
    ///
    /// A marker [`kill_all`](Self::kill_all) or [`shutdown`](Self::shutdown)
    /// is sweeping stays tracked until that sweep returns, and is released
    /// then, so it is no longer tracked once either has returned.
    pub fn release(&self, marker: &Marker) -> bool {
        let mut state = self.state();
        let released = state.markers.remove(marker);
        self.publish(&state);
        released
    }

    /// Send every registered group the SIGINT a terminal's Ctrl-C would, so
    /// each run can wind down and clean up what it started in sessions of its
    /// own, as often as the host is sent one: a second Ctrl-C reaches a run
    /// that caught the first. The groups stay registered, and a group
    /// registered afterwards is left alone unless a
    /// [shutdown](Self::shutdown) has begun.
    ///
    /// # Errors
    ///
    /// The first [`ExecutorError::ProcessGroupFailed`] among the groups that
    /// refused the interrupt, once every group has been sent it.
    pub fn interrupt_all(&self) -> Result<(), ExecutorError> {
        let state = self.state();
        Self::first_failure(state.groups.values().map(ProcessGroup::interrupt))
    }

    /// Whether a [`shutdown`](Self::shutdown) has begun, or
    /// [`kill_all`](Self::kill_all) has run, after which a host should start
    /// no new runs: each one would be interrupted, or killed, as it
    /// registers.
    pub fn is_stopping(&self) -> bool {
        let state = self.state();
        state.stopping || state.killed
    }

    /// Whether [`kill_all`](Self::kill_all) or [`shutdown`](Self::shutdown)
    /// has killed the registry, after which it tracks no marker and registers
    /// no group.
    pub fn is_killed(&self) -> bool {
        self.state().killed
    }

    /// Kill every registered group, and every group registered from now on,
    /// then sweep every tracked marker with [`Marker::kill`]. Each marker is
    /// released once its sweep returns, whether or not the sweep succeeded:
    /// a failed sweep is reported here, and a marker kept would hold
    /// [`emptied`](Self::emptied) open for good. Once this returns, every
    /// marker tracked when it was called has been swept, even one another
    /// call is still sweeping.
    ///
    /// A sweep blocks the calling thread; in async code, call
    /// [`shutdown`](Self::shutdown), or call this from
    /// [`tokio::task::spawn_blocking`].
    ///
    /// # Errors
    ///
    /// The first failure, once every group has been signalled and every
    /// marker swept: [`GroupRegistryError::Signal`] when a group refused the
    /// kill, and [`GroupRegistryError::Sweep`] when a sweep failed.
    pub fn kill_all(&self) -> Result<(), GroupRegistryError> {
        let (signalled, markers) = self.kill_groups();
        let swept = Self::first_failure(markers.iter().map(|marker| {
            let swept = marker.kill();
            self.release(marker);
            swept
        }));
        signalled
            .map_err(GroupRegistryError::Signal)
            .and(swept.map_err(GroupRegistryError::Sweep))
    }

    /// [Interrupt](Self::interrupt_all) every group, and every group
    /// registered from now on, give the runs up to `grace` to
    /// [empty](Self::emptied) the registry, then [kill](Self::kill_all)
    /// whatever is left, sweeping its markers on the runtime's blocking pool.
    ///
    /// Dropping the future within the grace leaves everything registered
    /// for a later [`kill_all`](Self::kill_all). Dropping it after the grace
    /// leaves the groups killed and the sweep running on the blocking pool,
    /// but the swept markers stay tracked until a
    /// [`kill_all`](Self::kill_all) sweeps them again. A runtime shutting
    /// down can cancel the sweep before it runs; the markers then stay
    /// tracked too, since sweeping them here would block the runtime.
    ///
    /// # Errors
    ///
    /// As for [`kill_all`](Self::kill_all),
    /// [`GroupRegistryError::Signal`] when a group refused the interrupt, and
    /// [`GroupRegistryError::Cancelled`] when the runtime cancelled the sweep.
    pub async fn shutdown(&self, grace: Duration) -> Result<(), GroupRegistryError> {
        let interrupted = self.stop();
        let _ = tokio::time::timeout(grace, self.emptied()).await;
        let (signalled, markers) = self.kill_groups();
        let swept = self.sweep_in_background(markers).await;
        interrupted
            .and(signalled)
            .map_err(GroupRegistryError::Signal)
            .and(swept)
    }

    /// Resolve once no group is registered and no marker tracked, whether the
    /// runs removed them or [`kill_all`](Self::kill_all) did. Cancel safe.
    pub async fn emptied(&self) {
        let _ = self.empty.subscribe().wait_for(|empty| *empty).await;
    }

    /// Whether no group is registered and no marker tracked.
    pub fn is_empty(&self) -> bool {
        self.state().is_empty()
    }

    /// Begin a shutdown and interrupt every group under the lock, so a group
    /// registers either before, and is interrupted here, or after, and is
    /// interrupted as it registers.
    fn stop(&self) -> Result<(), ExecutorError> {
        let mut state = self.state();
        state.stopping = true;
        Self::first_failure(state.groups.values().map(ProcessGroup::interrupt))
    }

    /// Mark the registry killed and kill and unregister every group under
    /// the lock, so nothing registers in between and nothing sees the
    /// registry empty before its groups are killed. Returns the tracked
    /// markers, which stay tracked until they are swept.
    fn kill_groups(&self) -> (Result<(), ExecutorError>, Vec<Marker>) {
        let mut state = self.state();
        state.killed = true;
        let groups = std::mem::take(&mut state.groups);
        let signalled = Self::first_failure(groups.values().map(ProcessGroup::kill));
        let markers = state.markers.iter().cloned().collect();
        self.publish(&state);
        (signalled, markers)
    }

    async fn sweep_in_background(&self, markers: Vec<Marker>) -> Result<(), GroupRegistryError> {
        if markers.is_empty() {
            return Ok(());
        }
        let sweeps = tokio::task::spawn_blocking(move || {
            markers
                .into_iter()
                .map(|marker| {
                    let swept = marker.kill();
                    (marker, swept)
                })
                .collect::<Vec<_>>()
        })
        .await;
        self.swept(sweeps)
    }

    fn swept(&self, sweeps: Result<Vec<Sweep>, JoinError>) -> Result<(), GroupRegistryError> {
        match sweeps {
            Ok(sweeps) => Self::first_failure(sweeps.into_iter().map(|(marker, swept)| {
                self.release(&marker);
                swept
            }))
            .map_err(GroupRegistryError::Sweep),
            Err(error) => match error.try_into_panic() {
                Ok(panic) => std::panic::resume_unwind(panic),
                Err(cancelled) => Err(GroupRegistryError::Cancelled(cancelled)),
            },
        }
    }

    /// The first failure among `results`, once every one has been produced.
    fn first_failure<T, E>(results: impl Iterator<Item = Result<T, E>>) -> Result<(), E> {
        let mut first = Ok(());
        for result in results {
            if let Err(error) = result
                && first.is_ok()
            {
                first = Err(error);
            }
        }
        first
    }

    fn publish(&self, state: &State) {
        let empty = state.is_empty();
        self.empty.send_if_modified(|current| {
            let changed = *current != empty;
            *current = empty;
            changed
        });
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
