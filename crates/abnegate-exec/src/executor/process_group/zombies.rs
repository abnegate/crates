//! Whether every member of a process group is an unreaped zombie, which
//! macOS refuses to signal.

use std::io;

use nix::errno::Errno;
use nix::libc;

/// `proc_listpids`'s filter for the members of one process group, from
/// `<sys/proc_info.h>`.
const PROC_PGRP_ONLY: u32 = 2;

/// `proc_pidinfo`'s argument that lets `PROC_PIDTBSDINFO` describe a zombie.
const FIND_ZOMBIES: u64 = 1;

/// How many members a listing makes room for at first.
const ROOM: usize = 64;

/// Whether every member of group `pgid` is a zombie, as all of them are once
/// a leader-only group's leader has exited unreaped. A group whose members
/// cannot be listed, or a member described, is not taken for one.
pub(super) fn only_zombies(pgid: i32) -> bool {
    members(pgid).is_ok_and(|members| members.into_iter().all(is_zombie_or_gone))
}

/// The pids of group `pgid`'s members, zombies included.
fn members(pgid: i32) -> io::Result<Vec<libc::pid_t>> {
    let group = u32::try_from(pgid).map_err(io::Error::other)?;
    let mut pids: Vec<libc::pid_t> = vec![0; ROOM];
    loop {
        let bytes =
            libc::c_int::try_from(size_of_val(pids.as_slice())).map_err(io::Error::other)?;
        Errno::clear();
        // SAFETY: `pids` is valid for `bytes` writable bytes, and
        // proc_listpids writes at most that many.
        let written =
            unsafe { libc::proc_listpids(PROC_PGRP_ONLY, group, pids.as_mut_ptr().cast(), bytes) };
        if written <= 0 && Errno::last_raw() != 0 {
            return Err(io::Error::last_os_error());
        }
        let listed = usize::try_from(written).unwrap_or_default() / size_of::<libc::pid_t>();
        if listed < pids.len() {
            pids.truncate(listed);
            return Ok(pids);
        }
        pids.resize(pids.len() * 2, 0);
    }
}

/// Whether `pid` is a zombie, or has gone since it was listed.
fn is_zombie_or_gone(pid: libc::pid_t) -> bool {
    // SAFETY: proc_bsdinfo holds only integers and byte arrays, all valid
    // zeroed.
    let mut information: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let Ok(size) = libc::c_int::try_from(size_of::<libc::proc_bsdinfo>()) else {
        return false;
    };
    // SAFETY: `information` is valid for `size` writable bytes, and
    // proc_pidinfo writes at most that many.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            FIND_ZOMBIES,
            (&raw mut information).cast(),
            size,
        )
    };
    if written == size {
        information.pbi_status == libc::SZOMB
    } else {
        Errno::last() == Errno::ESRCH
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::process::Stdio;
    use std::time::Duration;
    use std::time::Instant;

    use super::*;
    use crate::executor::sleeper::Sleeper;
    use crate::executor::sleeper::exits_on_close;

    const PATIENCE: Duration = Duration::from_secs(60);

    #[test]
    fn a_running_group_is_not_taken_for_zombies() {
        let sleeper = Sleeper::start();

        assert!(!only_zombies(sleeper.group().pgid()));
    }

    #[test]
    fn a_group_left_with_its_unreaped_leader_is_only_zombies() {
        let mut leader = exits_on_close();
        let pgid = i32::try_from(leader.id()).unwrap();
        drop(leader.stdin.take());
        let started = Instant::now();
        while !only_zombies(pgid) && started.elapsed() < PATIENCE {
            std::thread::sleep(Duration::from_millis(20));
        }

        let zombies = only_zombies(pgid);

        assert_eq!(leader.wait().expect("the leader is reaped").code(), Some(3));
        assert!(
            zombies,
            "an exited leader's group was taken for a running one"
        );
    }

    #[test]
    fn a_running_member_beside_a_zombie_leader_is_seen() {
        let mut leader = exits_on_close();
        let pgid = i32::try_from(leader.id()).unwrap();
        let mut member = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .process_group(pgid)
            .spawn()
            .expect("the member starts");
        drop(leader.stdin.take());
        let started = Instant::now();
        while members(pgid).is_ok_and(|members| {
            !members
                .iter()
                .any(|&pid| pid == pgid && is_zombie_or_gone(pid))
        }) && started.elapsed() < PATIENCE
        {
            std::thread::sleep(Duration::from_millis(20));
        }

        let zombies = only_zombies(pgid);

        let _ = member.kill();
        let _ = member.wait();
        let _ = leader.wait();
        assert!(
            !zombies,
            "a group with a running member was taken for zombies"
        );
    }
}
