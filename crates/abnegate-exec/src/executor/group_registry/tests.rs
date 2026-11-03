use std::os::unix::process::ExitStatusExt;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Barrier;
use std::time::Duration;

use nix::sys::signal::Signal;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::process::Command;

use super::run::Run;
use super::*;
use crate::executor::marker::straggler::Behaviour;
use crate::executor::marker::straggler::EXIT_DEADLINE;
use crate::executor::marker::straggler::Straggler;
use crate::executor::marker::straggler::VARIABLE;
use crate::executor::marker::straggler::exits_within;
use crate::executor::marker::straggler::is_running;
use crate::executor::marker::straggler::stop_if_running;

/// Leaves `sleep` running in the background, after printing its pid.
const BACKGROUND_SLEEP: &str = "sleep 300 & echo $!; exec sleep 300";

/// Like [`BACKGROUND_SLEEP`], but both processes ignore SIGINT.
const INTERRUPT_IGNORING_LEADER: &str = "trap '' INT; sleep 300 & echo $!; exec sleep 300";

/// Like [`BACKGROUND_SLEEP`], but the leader exits with status 7 on SIGINT
/// while its background process ignores it from the moment it is forked, so
/// an interrupt that lands before it first runs cannot end it. Perl, because
/// a shell cannot trap a signal it inherited as ignored.
const INTERRUPTIBLE_LEADER: &str = concat!(
    r#"exec perl -e '$| = 1; $SIG{INT} = "IGNORE"; "#,
    r#"defined(my $pid = fork) or die; "#,
    r#"if (!$pid) { exec "sleep", "300" } "#,
    r#"$SIG{INT} = sub { exit 7 }; print "$pid\n"; sleep 300'"#,
);

/// Prints its own pid, then a line for every SIGINT it receives.
const INTERRUPT_COUNTING_LEADER: &str = concat!(
    r#"exec perl -e '$| = 1; $SIG{INT} = sub { print "interrupted\n" }; "#,
    r#"print "$$\n"; sleep 300 while 1'"#,
);

/// How many groups the concurrency tests register at once.
const CONTENDERS: usize = 8;

/// How long a test lets a process run before it checks that nothing
/// signalled it.
const SETTLE: Duration = Duration::from_millis(200);

/// Kills a background process a failed test left running.
struct Stray(u32);

impl Drop for Stray {
    fn drop(&mut self) {
        stop_if_running(self.0);
    }
}

/// A shell running `script`, which prints the pid of a background process
/// first.
fn group_command(script: &str) -> Command {
    let mut command = Command::new("sh");
    command
        .args(["-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    command
}

/// A run's command: a shell that leaves a straggler acting out `behaviour`
/// running in the background, which prints its pid first.
fn straggling(behaviour: Behaviour) -> Command {
    Command::from(Straggler::behind_a_shell(behaviour))
}

/// The first pid the leader prints, skipping what a test harness prints
/// around it.
async fn background_pid(leader: &mut Child) -> u32 {
    let mut lines = BufReader::new(leader.stdout.take().expect("the leader's output")).lines();
    loop {
        let line = lines
            .next_line()
            .await
            .expect("the leader's output reads")
            .expect("the leader printed no pid");
        if let Ok(pid) = line.trim().parse() {
            return pid;
        }
    }
}

/// Spawn `script` leading a process group of its own, outside any run.
async fn spawn_group(script: &str) -> (Child, u32, ProcessGroup, Stray) {
    let mut leader = group_command(script)
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .expect("the shell starts");
    let group = ProcessGroup::try_from(leader.id().expect("the leader runs"))
        .expect("a child leading its own group");
    let background = background_pid(&mut leader).await;
    (leader, background, group, Stray(background))
}

async fn assert_killed(leader: &mut Child) {
    let status = tokio::time::timeout(EXIT_DEADLINE, leader.wait())
        .await
        .expect("the leader outlived its group's kill")
        .expect("the leader is reaped");
    assert_eq!(
        status.signal(),
        Some(Signal::SIGKILL as i32),
        "the leader exited with {status} instead of being killed"
    );
}

/// Wait for the leader of `group` to exit, leaving it unreaped, as a run
/// does before it kills what the leader left in its group.
async fn leader_exited(group: &ProcessGroup) {
    tokio::time::timeout(EXIT_DEADLINE, group.leader_exited())
        .await
        .expect("the leader outlived its interrupt")
        .expect("the leader is watched");
}

/// Release `group` and reap its leader, which must have ended on the
/// interrupt.
async fn assert_interrupted(leader: &mut Child, group: &ProcessGroup) {
    group.release();
    let status = leader.wait().await.expect("the leader is reaped");
    assert_eq!(status.code(), Some(7), "the leader exited with {status}");
}

#[test]
fn every_handle_to_the_global_registry_is_the_same_registry() {
    assert!(Arc::ptr_eq(
        &GroupRegistry::global(),
        &GroupRegistry::global()
    ));
}

#[tokio::test]
async fn kill_all_kills_every_registered_group() {
    let registry = GroupRegistry::new();
    let (mut first, first_background, first_group, _first_stray) =
        spawn_group(BACKGROUND_SLEEP).await;
    let (mut second, second_background, second_group, _second_stray) =
        spawn_group(BACKGROUND_SLEEP).await;
    registry.insert(first_group).unwrap();
    registry.insert(second_group).unwrap();

    registry.kill_all().unwrap();

    for (leader, background) in [
        (&mut first, first_background),
        (&mut second, second_background),
    ] {
        assert_killed(leader).await;
        assert!(
            exits_within(background),
            "background process {background} survived kill_all"
        );
    }
}

#[tokio::test]
async fn kill_all_empties_the_registry() {
    let registry = GroupRegistry::new();
    let (mut leader, background, group, _stray) = spawn_group(BACKGROUND_SLEEP).await;
    registry.insert(group.clone()).unwrap();
    assert!(registry.track(Marker::new(VARIABLE).unwrap()));

    registry.kill_all().unwrap();

    assert_killed(&mut leader).await;
    assert!(exits_within(background));
    assert!(registry.is_killed());
    assert!(
        registry.is_empty(),
        "a killed group must not be signalled again once the OS reuses its id"
    );
    assert!(
        !registry.kill(&group).unwrap(),
        "a group kill_all took was killed again"
    );
}

#[tokio::test]
async fn kill_all_kills_marked_processes_of_every_run() {
    let registry = GroupRegistry::new();
    let (mut first, _first_run) =
        Run::spawn(&mut straggling(Behaviour::Escapes), &registry).unwrap();
    let (mut second, _second_run) =
        Run::spawn(&mut straggling(Behaviour::Escapes), &registry).unwrap();
    let escaped = [
        background_pid(&mut first).await,
        background_pid(&mut second).await,
    ];

    registry.kill_all().unwrap();

    let survivors: Vec<u32> = escaped
        .into_iter()
        .filter(|&pid| !exits_within(pid))
        .collect();
    for &pid in &survivors {
        stop_if_running(pid);
    }
    assert!(
        survivors.is_empty(),
        "processes {survivors:?} that left their runs' groups survived kill_all"
    );
    assert_killed(&mut first).await;
    assert_killed(&mut second).await;
}

#[tokio::test]
async fn kill_all_lets_shutdown_stop_waiting_on_runs() {
    let registry = GroupRegistry::new();
    let (mut leader, _run) = Run::spawn(&mut group_command(BACKGROUND_SLEEP), &registry).unwrap();

    registry.kill_all().unwrap();

    tokio::time::timeout(EXIT_DEADLINE, registry.emptied())
        .await
        .expect("emptied must resolve once kill_all has swept every run");
    assert_killed(&mut leader).await;
}

#[tokio::test]
async fn kill_all_reaches_a_runs_group_and_what_it_left_in_it() {
    let registry = GroupRegistry::new();
    let (mut leader, _run) = Run::spawn(&mut group_command(BACKGROUND_SLEEP), &registry).unwrap();
    let background = background_pid(&mut leader).await;
    let _stray = Stray(background);

    registry.kill_all().unwrap();

    assert_killed(&mut leader).await;
    assert!(
        exits_within(background),
        "background process {background} survived kill_all"
    );
}

#[test]
fn a_killed_registry_tracks_no_marker() {
    let registry = GroupRegistry::new();
    registry.kill_all().unwrap();

    assert!(registry.is_killed());
    assert!(
        !registry.track(Marker::new(VARIABLE).unwrap()),
        "a killed registry tracked a marker"
    );
    assert!(registry.is_empty());
}

#[tokio::test]
async fn a_group_inserted_after_kill_all_is_killed_instead_of_registered() {
    let registry = GroupRegistry::new();
    let (mut leader, background, group, _stray) = spawn_group(BACKGROUND_SLEEP).await;
    registry.kill_all().unwrap();

    registry.insert(group).unwrap();

    assert_killed(&mut leader).await;
    assert!(exits_within(background));
    assert!(
        registry.is_empty(),
        "a group killed on insert was registered"
    );
}

#[tokio::test]
async fn kill_signals_only_registered_groups() {
    let registry = GroupRegistry::new();
    let (mut leader, background, group, _stray) = spawn_group(BACKGROUND_SLEEP).await;

    assert!(!registry.kill(&group).unwrap());
    tokio::time::sleep(SETTLE).await;
    assert!(
        leader.try_wait().unwrap().is_none(),
        "an unregistered group must not be signalled"
    );

    registry.insert(group.clone()).unwrap();
    assert!(registry.kill(&group).unwrap());
    assert_killed(&mut leader).await;
    assert!(
        exits_within(background),
        "background process {background} survived kill"
    );
    assert!(registry.is_empty());
}

#[tokio::test]
async fn remove_unregisters_a_group_without_signalling_it() {
    let registry = GroupRegistry::new();
    let (mut leader, background, group, _stray) = spawn_group(BACKGROUND_SLEEP).await;
    registry.insert(group.clone()).unwrap();

    assert!(registry.remove(&group));
    registry.kill_all().unwrap();
    tokio::time::sleep(SETTLE).await;

    assert!(registry.is_empty());
    assert!(
        leader.try_wait().unwrap().is_none(),
        "a removed group was signalled"
    );
    assert!(is_running(background));
    group.kill_until_gone().await.unwrap();
    assert_killed(&mut leader).await;
    assert!(exits_within(background));
}

#[tokio::test]
async fn a_handle_built_apart_for_the_same_id_leaves_the_registered_group_alone() {
    let registry = GroupRegistry::new();
    let (mut leader, background, group, _stray) = spawn_group(BACKGROUND_SLEEP).await;
    let apart = ProcessGroup::try_from(leader.id().unwrap()).unwrap();
    registry.insert(group.clone()).unwrap();

    assert!(!registry.remove(&apart));
    assert!(!registry.kill(&apart).unwrap());
    tokio::time::sleep(SETTLE).await;

    assert!(
        leader.try_wait().unwrap().is_none(),
        "the registered group was killed through another group's handle"
    );
    assert!(
        !registry.is_empty(),
        "another group's handle unregistered it"
    );
    registry.kill_all().unwrap();
    assert_killed(&mut leader).await;
    assert!(exits_within(background));
}

#[tokio::test]
async fn a_handle_built_apart_for_a_registered_id_replaces_it() {
    let registry = GroupRegistry::new();
    let (mut leader, _background, earlier, _stray) = spawn_group(BACKGROUND_SLEEP).await;
    let later = ProcessGroup::try_from(leader.id().unwrap()).unwrap();
    registry.insert(earlier.clone()).unwrap();

    registry.insert(later.clone()).unwrap();

    assert!(
        !registry.remove(&earlier),
        "the replaced handle unregistered the group that replaced it"
    );
    assert!(registry.remove(&later));
    assert!(registry.is_empty());
    later.kill_until_gone().await.unwrap();
    assert_killed(&mut leader).await;
}

#[tokio::test]
async fn interrupt_all_interrupts_without_forgetting_the_group() {
    let registry = GroupRegistry::new();
    let (mut leader, background, group, _stray) = spawn_group(INTERRUPTIBLE_LEADER).await;
    registry.insert(group.clone()).unwrap();

    registry.interrupt_all().unwrap();

    leader_exited(&group).await;
    assert!(
        !registry.is_stopping(),
        "a ctrl-c passed on was taken for a shutdown"
    );
    assert!(
        !registry.is_empty(),
        "an interrupted group was unregistered"
    );
    registry.kill_all().unwrap();
    assert_interrupted(&mut leader, &group).await;
    assert!(
        exits_within(background),
        "kill_all must still reach whatever ignored the interrupt"
    );
}

#[tokio::test]
async fn a_group_registered_after_interrupt_all_is_left_running() {
    let registry = GroupRegistry::new();
    registry.interrupt_all().unwrap();
    let (mut leader, background, group, _stray) = spawn_group(INTERRUPTIBLE_LEADER).await;

    registry.insert(group.clone()).unwrap();

    let exited = tokio::time::timeout(SETTLE, group.leader_exited()).await;
    registry.kill_all().unwrap();
    assert!(
        exited.is_err(),
        "a group registered after a ctrl-c, with no shutdown begun, was interrupted"
    );
    assert_killed(&mut leader).await;
    assert!(exits_within(background));
}

#[tokio::test]
async fn a_group_registered_once_a_shutdown_has_begun_is_interrupted() {
    let registry = GroupRegistry::new();
    assert!(!registry.is_stopping());
    registry.stop().unwrap();
    assert!(registry.is_stopping());
    let (mut leader, background, group, _stray) = spawn_group(INTERRUPTIBLE_LEADER).await;

    registry.insert(group.clone()).unwrap();

    leader_exited(&group).await;
    registry.kill_all().unwrap();
    assert_interrupted(&mut leader, &group).await;
    assert!(exits_within(background));
}

#[tokio::test]
async fn every_interrupt_all_reaches_every_group() {
    let registry = GroupRegistry::new();
    let mut leader = group_command(INTERRUPT_COUNTING_LEADER)
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let group = ProcessGroup::try_from(leader.id().unwrap()).unwrap();
    let mut lines = BufReader::new(leader.stdout.take().unwrap()).lines();
    let announced = lines.next_line().await.unwrap();
    assert_eq!(announced, Some(group.pgid().to_string()));
    registry.insert(group).unwrap();

    let mut received = Vec::new();
    for _ in 0..2 {
        registry.interrupt_all().unwrap();
        let line = tokio::time::timeout(EXIT_DEADLINE, lines.next_line()).await;
        received.push(line.ok().and_then(Result::ok).flatten());
    }

    registry.kill_all().unwrap();
    assert_killed(&mut leader).await;
    assert_eq!(
        received,
        [
            Some("interrupted".to_string()),
            Some("interrupted".to_string())
        ],
        "a second ctrl-c never reached the run"
    );
}

#[tokio::test]
async fn shutdown_kills_what_ignores_the_interrupt() {
    let registry = GroupRegistry::new();
    let (mut leader, background, group, _stray) = spawn_group(INTERRUPT_IGNORING_LEADER).await;
    registry.insert(group).unwrap();

    registry.shutdown(SETTLE).await.unwrap();

    assert_killed(&mut leader).await;
    assert!(exits_within(background));
    assert!(registry.is_empty());
}

#[tokio::test]
async fn shutdown_returns_once_interrupted_runs_end() {
    let registry = GroupRegistry::new();
    let (mut leader, background, group, _stray) = spawn_group(INTERRUPTIBLE_LEADER).await;
    registry.insert(group.clone()).unwrap();
    let run_ends_on_interrupt = async {
        leader_exited(&group).await;
        registry.kill(&group).unwrap();
        assert_interrupted(&mut leader, &group).await;
    };

    let shutdown = tokio::time::timeout(EXIT_DEADLINE, async {
        tokio::join!(registry.shutdown(2 * EXIT_DEADLINE), run_ends_on_interrupt)
    })
    .await;

    let (stopped, ()) = shutdown.expect("shutdown waited out its grace after the run ended");
    stopped.unwrap();
    assert!(exits_within(background));
}

#[tokio::test]
async fn shutdown_sweeps_the_markers_still_tracked() {
    let registry = GroupRegistry::new();
    let marker = Marker::new(VARIABLE).unwrap();
    let escaped = Straggler::start(&mut Straggler::marked(Behaviour::Escapes, &marker), 1);
    assert!(registry.track(marker));

    registry.shutdown(SETTLE).await.unwrap();

    assert!(
        escaped.stops(),
        "a process carrying a tracked marker outlived shutdown"
    );
    assert!(registry.is_empty());
}

/// A runtime shutting down cancels a sweep queued on its blocking pool, and
/// sweeping again in its place would block a worker of that runtime.
#[tokio::test]
async fn a_cancelled_sweep_is_reported_and_leaves_its_markers_tracked() {
    let registry = GroupRegistry::new();
    let marker = Marker::new(VARIABLE).unwrap();
    let mut escaped = Straggler::start(&mut Straggler::marked(Behaviour::Escapes, &marker), 1);
    assert!(registry.track(marker));
    let cancelled = tokio::spawn(std::future::pending::<Vec<Sweep>>());
    cancelled.abort();
    let cancellation = cancelled.await.expect_err("an aborted task");

    let swept = registry.swept(Err(cancellation));

    let running = escaped.is_running();
    escaped.kill();
    assert!(
        matches!(swept, Err(GroupRegistryError::Cancelled(_))),
        "{swept:?}"
    );
    assert!(running, "the cancelled sweep was run again on the runtime");
    assert!(
        !registry.is_empty(),
        "a marker no sweep reached was released"
    );
}

#[test]
fn kill_all_spares_the_processes_of_a_released_marker() {
    let registry = GroupRegistry::new();
    let marker = Marker::new(VARIABLE).unwrap();
    let mut straggler = Straggler::start(&mut Straggler::marked(Behaviour::Escapes, &marker), 1);
    assert!(registry.track(marker.clone()));

    assert!(registry.release(&marker));
    assert!(!registry.release(&marker));
    registry.kill_all().unwrap();
    std::thread::sleep(SETTLE);

    assert!(
        straggler.is_running(),
        "a process whose marker was released was swept"
    );
}

#[tokio::test]
async fn a_marker_being_swept_keeps_the_registry_from_emptying() {
    let registry = GroupRegistry::new();
    let marker = Marker::new(VARIABLE).unwrap();
    let escaped = Straggler::start(&mut Straggler::marked(Behaviour::Escapes, &marker), 1);
    assert!(registry.track(marker.clone()));

    let (signalled, sweeping) = registry.kill_groups();

    signalled.unwrap();
    assert_eq!(sweeping, vec![marker]);
    assert!(
        !registry.is_empty(),
        "the registry emptied before the marker was swept"
    );
    let early = tokio::time::timeout(SETTLE, registry.emptied()).await;
    assert!(
        early.is_err(),
        "emptied resolved before the marker was swept"
    );
    registry.kill_all().unwrap();
    assert!(
        escaped.stops(),
        "a kill_all during another sweep returned without sweeping its marker"
    );
    assert!(registry.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_all_racing_a_shutdown_returns_once_the_markers_are_swept() {
    let registry = Arc::new(GroupRegistry::new());
    let marker = Marker::new(VARIABLE).unwrap();
    let escaped = Straggler::start(
        &mut Straggler::marked(Behaviour::IgnoresTerminate, &marker),
        1,
    );
    let pid = escaped.pid();
    assert!(registry.track(marker));

    let shutting_down = Arc::clone(&registry);
    let shutdown = tokio::spawn(async move { shutting_down.shutdown(Duration::ZERO).await });
    let killing = Arc::clone(&registry);
    tokio::task::spawn_blocking(move || killing.kill_all())
        .await
        .unwrap()
        .unwrap();

    assert!(
        exits_within(pid),
        "process {pid} outlived a kill_all that had returned"
    );
    shutdown.await.unwrap().unwrap();
    assert!(registry.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_group_inserted_while_kill_all_runs_is_killed() {
    let registry = Arc::new(GroupRegistry::new());
    let barrier = Arc::new(Barrier::new(CONTENDERS + 1));
    let mut spawned = Vec::new();
    for _ in 0..CONTENDERS {
        spawned.push(spawn_group(BACKGROUND_SLEEP).await);
    }

    let mut inserts = Vec::new();
    for (_, _, group, _) in &spawned {
        let (registry, barrier, group) =
            (Arc::clone(&registry), Arc::clone(&barrier), group.clone());
        inserts.push(tokio::task::spawn_blocking(move || {
            barrier.wait();
            registry.insert(group)
        }));
    }
    let killing = Arc::clone(&registry);
    let kill = tokio::task::spawn_blocking(move || {
        barrier.wait();
        killing.kill_all()
    });
    for insert in inserts {
        insert.await.unwrap().unwrap();
    }
    kill.await.unwrap().unwrap();

    for (leader, background, _, _stray) in &mut spawned {
        assert_killed(leader).await;
        assert!(exits_within(*background));
    }
    assert!(registry.is_empty(), "a group registered after the kill");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_group_inserted_while_a_shutdown_begins_is_interrupted() {
    let registry = Arc::new(GroupRegistry::new());
    let barrier = Arc::new(Barrier::new(CONTENDERS + 1));
    let mut spawned = Vec::new();
    for _ in 0..CONTENDERS {
        spawned.push(spawn_group(INTERRUPTIBLE_LEADER).await);
    }

    let mut inserts = Vec::new();
    for (_, _, group, _) in &spawned {
        let (registry, barrier, group) =
            (Arc::clone(&registry), Arc::clone(&barrier), group.clone());
        inserts.push(tokio::task::spawn_blocking(move || {
            barrier.wait();
            registry.insert(group)
        }));
    }
    let interrupting = Arc::clone(&registry);
    let interrupt = tokio::task::spawn_blocking(move || {
        barrier.wait();
        interrupting.stop()
    });
    for insert in inserts {
        insert.await.unwrap().unwrap();
    }
    interrupt.await.unwrap().unwrap();

    for (_, _, group, _) in &spawned {
        leader_exited(group).await;
    }
    registry.kill_all().unwrap();
    for (leader, background, group, _stray) in &mut spawned {
        assert_interrupted(leader, group).await;
        assert!(exits_within(*background));
    }
}

#[tokio::test]
async fn emptied_waits_for_every_group_to_be_killed() {
    let registry = GroupRegistry::new();
    let (mut leader, background, group, _stray) = spawn_group(BACKGROUND_SLEEP).await;
    registry.insert(group.clone()).unwrap();

    let early = tokio::time::timeout(SETTLE, registry.emptied()).await;
    assert!(early.is_err(), "emptied resolved while a group was live");
    assert!(is_running(background));

    registry.kill(&group).unwrap();

    tokio::time::timeout(EXIT_DEADLINE, registry.emptied())
        .await
        .expect("emptied must resolve once the last group is killed");
    assert_killed(&mut leader).await;
}

#[tokio::test]
async fn emptied_waits_for_a_run_still_sweeping() {
    let registry = GroupRegistry::new();
    let (mut leader, mut run) =
        Run::spawn(&mut straggling(Behaviour::IgnoresTerminate), &registry).unwrap();
    let escaped = background_pid(&mut leader).await;

    let ((), emptied_while_sweeping) = tokio::join!(run.finish(), async {
        tokio::time::timeout(Duration::from_millis(500), registry.emptied())
            .await
            .is_ok()
    });

    let running = stop_if_running(escaped);
    assert!(
        !emptied_while_sweeping,
        "emptied resolved while a run was still sweeping the processes it left behind"
    );
    assert!(
        !running,
        "process {escaped} that ignores SIGTERM outlived the sweep's grace"
    );
    tokio::time::timeout(EXIT_DEADLINE, registry.emptied())
        .await
        .expect("emptied must resolve once the run's sweep finishes");
    assert_killed(&mut leader).await;
}

#[tokio::test]
async fn a_failed_spawn_leaves_nothing_for_shutdown_to_wait_on() {
    let registry = GroupRegistry::new();
    let directory = tempfile::tempdir().unwrap();

    let spawned = Run::spawn(&mut Command::new(directory.path().join("agent")), &registry);

    assert!(spawned.is_err(), "a missing program must fail to spawn");
    tokio::time::timeout(EXIT_DEADLINE, registry.emptied())
        .await
        .expect("a failed spawn must not leave the registry waiting on its run");
}

#[tokio::test]
async fn a_run_spawned_once_the_registry_is_killed_is_refused() {
    let registry = GroupRegistry::new();
    registry.kill_all().unwrap();

    let spawned = Run::spawn(&mut group_command(BACKGROUND_SLEEP), &registry);

    assert!(spawned.is_err(), "a run started after kill_all");
    assert!(registry.is_empty());
}

#[tokio::test]
async fn wait_kills_the_group_and_returns_the_leader_status() {
    let registry = GroupRegistry::new();
    let (mut leader, mut run) =
        Run::spawn(&mut group_command("sleep 300 & echo $!; exit 3"), &registry).unwrap();
    let background = background_pid(&mut leader).await;
    let _stray = Stray(background);

    let status = tokio::time::timeout(EXIT_DEADLINE, run.wait(&mut leader))
        .await
        .expect("wait must return once the leader exits")
        .unwrap();

    assert_eq!(status.code(), Some(3));
    assert!(
        exits_within(background),
        "background process {background} outlived its leader"
    );
    run.finish().await;
    assert!(registry.is_empty(), "a finished run must unregister");
}

#[tokio::test]
async fn wait_leaves_a_running_leader_alone() {
    let registry = GroupRegistry::new();
    let (mut leader, mut run) =
        Run::spawn(&mut group_command(BACKGROUND_SLEEP), &registry).unwrap();
    let background = background_pid(&mut leader).await;
    let _stray = Stray(background);

    let early = tokio::time::timeout(SETTLE, run.wait(&mut leader)).await;

    assert!(early.is_err(), "wait returned while the leader was running");
    assert!(
        leader.try_wait().unwrap().is_none(),
        "wait killed a running leader"
    );
    assert!(is_running(background));
    drop(run);
    assert_killed(&mut leader).await;
}

#[tokio::test]
async fn drop_kills_the_group_and_unregisters_it() {
    let registry = GroupRegistry::new();
    let (mut leader, run) = Run::spawn(&mut group_command(BACKGROUND_SLEEP), &registry).unwrap();
    let background = background_pid(&mut leader).await;
    let _stray = Stray(background);

    drop(run);

    assert_killed(&mut leader).await;
    assert!(
        exits_within(background),
        "background process {background} survived the run's drop"
    );
    assert!(registry.is_empty(), "a dropped run must unregister");
}

#[tokio::test]
async fn drop_kills_a_marked_process_in_a_session_of_its_own() {
    let registry = GroupRegistry::new();
    let (mut leader, run) = Run::spawn(&mut straggling(Behaviour::Escapes), &registry).unwrap();
    let escaped = background_pid(&mut leader).await;

    drop(run);

    let exited = exits_within(escaped);
    if !exited {
        stop_if_running(escaped);
    }
    assert!(
        exited,
        "process {escaped} that left the group outlived the run's drop"
    );
    assert_killed(&mut leader).await;
}

#[tokio::test]
async fn finish_terminates_a_marked_process_in_a_session_of_its_own() {
    let registry = GroupRegistry::new();
    let (mut leader, mut run) = Run::spawn(&mut straggling(Behaviour::Escapes), &registry).unwrap();
    let escaped = background_pid(&mut leader).await;

    run.finish().await;

    let running = stop_if_running(escaped);
    assert!(
        !running,
        "process {escaped} that left the group outlived finish"
    );
    assert!(registry.is_empty(), "a finished run must unregister");
    assert_killed(&mut leader).await;
}
