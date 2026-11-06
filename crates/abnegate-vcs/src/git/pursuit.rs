use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::SendError;
use std::sync::mpsc::Sender;
use std::sync::mpsc::channel;
use std::time::Duration;
use std::time::Instant;

use nix::unistd::Pid;

use crate::git::leader::Leader;

/// The pause between one round of kills and reaps and the next.
const ROUND: Duration = Duration::from_millis(1);

/// How long a kill goes on repeating itself for a group that still answers.
/// Outside Linux a group signal reaches only the members present when it
/// lands, so a child a member was forking at that instant survives a single
/// kill. A group stops answering as soon as its members are gone, so this
/// bounds only a member that cannot die; on a loaded host, members already
/// sent SIGKILL can go on answering for a second or more.
#[cfg(not(target_os = "linux"))]
const BUDGET: Duration = Duration::from_secs(10);

/// Linux restarts a fork that a group signal interrupts, so its child is
/// never left out of the first kill and nothing is repeated.
#[cfg(target_os = "linux")]
const BUDGET: Duration = Duration::ZERO;

/// The name of the one thread that finishes every dropped group's teardown.
const REAPER: &str = "abnegate-vcs-group-reaper";

/// Where dropped groups are sent, once the reaper is running.
static REAPING: Mutex<Option<Sender<Pursuit>>> = Mutex::new(None);

/// What is left of a dropped group's teardown once its first kill is sent:
/// repeating the kill while the group still answers, then reaping its
/// leader. The leader goes unreaped until the kills stop, so the group's
/// identifier cannot be handed to another group while it is still being
/// signalled.
pub(crate) struct Pursuit {
    id: Pid,
    kill: fn(Pid) -> nix::Result<()>,
    leader: Box<dyn Leader>,
    repeating: Option<Instant>,
}

impl Pursuit {
    /// The teardown of the group `id` led by `leader`, which repeats `kill`
    /// for as long as the group still `answered` it.
    pub(crate) fn new(
        id: Pid,
        kill: fn(Pid) -> nix::Result<()>,
        leader: Box<dyn Leader>,
        answered: bool,
    ) -> Self {
        Self {
            id,
            kill,
            leader,
            repeating: answered.then(|| Instant::now() + BUDGET),
        }
    }

    /// Hand the teardown to the reaper, starting it if it is not running,
    /// so the dropping thread never waits on a group. Where the reaper
    /// cannot be started, the leader is reaped only if it has already exited.
    pub(crate) fn hand_off(self) {
        let mut reaping = REAPING.lock().unwrap_or_else(PoisonError::into_inner);
        let unsent = match reaping.as_ref() {
            Some(sender) => match sender.send(self) {
                Ok(()) => return,
                Err(SendError(unsent)) => unsent,
            },
            None => self,
        };
        let (sender, receiver) = channel();
        let started = std::thread::Builder::new()
            .name(REAPER.to_string())
            .spawn(move || Self::chase(&receiver));
        *reaping = None;
        match started {
            Ok(_) => match sender.send(unsent) {
                Ok(()) => *reaping = Some(sender),
                Err(SendError(unsent)) => unsent.abandon(),
            },
            Err(error) => {
                tracing::warn!(%error, group = unsent.id.as_raw(), "Could not start the Git process group reaper");
                unsent.abandon();
            }
        }
    }

    /// Give the teardown up, reaping the leader only if it has already exited.
    fn abandon(mut self) {
        self.leader.reaped();
    }

    /// Every teardown `receiver` hands over, run a round at a time until
    /// none is left, then waited for again.
    fn chase(receiver: &Receiver<Self>) {
        let mut pending = Vec::new();
        loop {
            if pending.is_empty() {
                let Ok(pursuit) = receiver.recv() else {
                    return;
                };
                pending.push(pursuit);
            }
            pending.extend(receiver.try_iter());
            std::thread::sleep(ROUND);
            pending.retain_mut(Self::advance);
        }
    }

    /// One round: the kill again while the group answers and the budget
    /// lasts, and once it stops, an attempt to reap the leader. Whether the
    /// teardown is still unfinished.
    fn advance(&mut self) -> bool {
        if let Some(deadline) = self.repeating {
            if Instant::now() < deadline && (self.kill)(self.id).is_ok() {
                return true;
            }
            self.repeating = None;
        }
        !self.leader.reaped()
    }
}
