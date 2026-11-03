use std::io;
use std::ptr;

use nix::errno::Errno;
use nix::libc;
use nix::sys::signal::Signal;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use nix::unistd::geteuid;

/// `proc_listpids`'s filter for the processes of one user, from
/// `<sys/proc_info.h>`.
const PROC_UID_ONLY: u32 = 4;

/// Room for the processes started between sizing the list and filling it.
const SLACK: usize = 64;

/// Lists the caller's processes and reads their environments through one
/// buffer, sized for the largest any process can have.
pub(super) struct Scanner {
    own: u32,
    arguments: Vec<u8>,
}

impl Scanner {
    pub(super) fn new() -> io::Result<Self> {
        Ok(Self {
            own: std::process::id(),
            arguments: argument_buffer()?,
        })
    }

    /// Processes of the caller's effective user, other than the caller, whose
    /// own environment holds `entry` exactly.
    pub(super) fn find(&mut self, entry: &[u8]) -> io::Result<Vec<u32>> {
        let mut pids = list()?;
        pids.retain(|&pid| pid > 0 && pid.cast_unsigned() != self.own);
        let processes = pids
            .into_iter()
            .filter(|&pid| carries(pid, entry, &mut self.arguments))
            .map(libc::pid_t::cast_unsigned)
            .collect();
        Ok(processes)
    }

    /// Send `signal` to `pid` if it still carries `entry`. Returns whether it
    /// was delivered.
    pub(super) fn deliver(&mut self, pid: u32, entry: &[u8], signal: Signal) -> io::Result<bool> {
        let Ok(target) = libc::pid_t::try_from(pid) else {
            return Ok(false);
        };
        if !carries(target, entry, &mut self.arguments) {
            return Ok(false);
        }
        match kill(Pid::from_raw(target), signal) {
            Ok(()) => Ok(true),
            Err(Errno::ESRCH) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}

fn list() -> io::Result<Vec<libc::pid_t>> {
    // SAFETY: a null buffer only asks proc_listpids for the byte size it needs.
    let estimate = unsafe { libc::proc_listpids(PROC_UID_ONLY, user(), ptr::null_mut(), 0) };
    let mut pids = vec![0; listed(estimate)? + SLACK];
    loop {
        let filled = fill(&mut pids)?;
        if filled < pids.len() {
            pids.truncate(filled);
            return Ok(pids);
        }
        pids.resize(pids.len() * 2, 0);
    }
}

/// Fill `pids` with those of the caller's effective user, returning how many
/// it holds.
fn fill(pids: &mut [libc::pid_t]) -> io::Result<usize> {
    let bytes = libc::c_int::try_from(size_of_val(pids)).map_err(io::Error::other)?;
    // SAFETY: `pids` is valid for `bytes` writable bytes, and proc_listpids
    // writes at most that many.
    let written =
        unsafe { libc::proc_listpids(PROC_UID_ONLY, user(), pids.as_mut_ptr().cast(), bytes) };
    listed(written)
}

/// The number of pids in the `bytes` proc_listpids returned. It returns 0 on
/// failure: a listing by the caller's own user always holds the caller.
fn listed(bytes: libc::c_int) -> io::Result<usize> {
    match usize::try_from(bytes) {
        Ok(bytes @ 1..) => Ok(bytes / size_of::<libc::pid_t>()),
        _ => Err(io::Error::last_os_error()),
    }
}

fn user() -> libc::uid_t {
    geteuid().as_raw()
}

/// A buffer that fits the arguments and environment of any process.
fn argument_buffer() -> io::Result<Vec<u8>> {
    let mut limit = [0; size_of::<libc::c_int>()];
    query(&mut [libc::CTL_KERN, libc::KERN_ARGMAX], &mut limit)?;
    let limit = usize::try_from(libc::c_int::from_ne_bytes(limit)).map_err(io::Error::other)?;
    Ok(vec![0; limit])
}

/// Whether the environment of process `pid` holds `entry`. Reading it fails
/// for zombies, processes that are gone and those of other users, so they
/// never match.
fn carries(pid: libc::pid_t, entry: &[u8], arguments: &mut [u8]) -> bool {
    query(&mut [libc::CTL_KERN, libc::KERN_PROCARGS2, pid], arguments)
        .ok()
        .and_then(|filled| arguments.get(..filled))
        .is_some_and(|arguments| environment(arguments).any(|variable| variable == entry))
}

/// The environment in the KERN_PROCARGS2 layout: argc as a native `c_int`,
/// the executable path, NUL padding up to the next pointer-aligned offset,
/// argc arguments, the environment up to an empty string, then the apple
/// strings, which must not match.
fn environment(arguments: &[u8]) -> impl Iterator<Item = &[u8]> {
    let (count, strings) = arguments
        .split_first_chunk()
        .and_then(|(count, strings)| {
            let count = usize::try_from(libc::c_int::from_ne_bytes(*count)).ok()?;
            let executable = strings.iter().position(|&byte| byte == 0)?;
            let start = (executable + 1).next_multiple_of(size_of::<usize>());
            let end = strings.iter().rposition(|&byte| byte == 0)?;
            Some((count, strings.get(start..end)?))
        })
        .unwrap_or_default();
    strings
        .split(|&byte| byte == 0)
        .skip(count)
        .take_while(|string| !string.is_empty())
}

/// Read sysctl `name` into `buffer`, returning how many bytes it filled.
fn query(name: &mut [libc::c_int], buffer: &mut [u8]) -> io::Result<usize> {
    let length = libc::c_uint::try_from(name.len()).map_err(io::Error::other)?;
    let mut filled = buffer.len();
    // SAFETY: `name` is valid for `length` ints and `buffer` for `filled`
    // writable bytes; sysctl writes at most that many, stores how many it
    // wrote in `filled`, and sets nothing.
    let result = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            length,
            buffer.as_mut_ptr().cast(),
            &mut filled,
            ptr::null_mut(),
            0,
        )
    };
    if result == 0 {
        Ok(filled)
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(executable: &str, arguments: &[&str], environment: &[&str]) -> Vec<u8> {
        let mut strings = executable.as_bytes().to_vec();
        strings.push(0);
        strings.resize(strings.len().next_multiple_of(size_of::<usize>()), 0);
        for string in arguments.iter().chain(environment) {
            strings.extend_from_slice(string.as_bytes());
            strings.push(0);
        }
        strings.push(0);
        strings.extend_from_slice(b"executable_path=/bin/agent\0ptr_munge=\0");
        let mut buffer = libc::c_int::try_from(arguments.len())
            .unwrap()
            .to_ne_bytes()
            .to_vec();
        buffer.extend_from_slice(&strings);
        buffer
    }

    fn variables(buffer: &[u8]) -> Vec<&str> {
        environment(buffer)
            .map(|variable| std::str::from_utf8(variable).unwrap())
            .collect()
    }

    #[test]
    fn the_environment_excludes_the_arguments_and_the_apple_strings() {
        let buffer = layout(
            "/bin/agent",
            &["agent", "RUN=argument"],
            &["HOME=/home", "RUN=marker"],
        );

        assert_eq!(variables(&buffer), ["HOME=/home", "RUN=marker"]);
    }

    #[test]
    fn an_empty_argument_is_not_mistaken_for_the_end_of_the_environment() {
        let buffer = layout("/bin/agent", &["agent", ""], &["RUN=marker"]);

        assert_eq!(variables(&buffer), ["RUN=marker"]);
    }

    #[test]
    fn a_truncated_or_empty_buffer_holds_no_environment() {
        assert!(variables(&[]).is_empty());
        assert!(variables(&[1, 0]).is_empty());
    }

    #[test]
    fn an_empty_program_name_is_not_mistaken_for_padding() {
        let buffer = layout("/bin/agent", &["", "--flag"], &["RUN=marker", "HOME=/home"]);

        assert_eq!(variables(&buffer), ["RUN=marker", "HOME=/home"]);
    }

    #[test]
    fn an_executable_path_that_ends_aligned_has_no_padding() {
        let executable = "/bin/ag";
        assert_eq!((executable.len() + 1) % size_of::<usize>(), 0);
        let buffer = layout(executable, &["", "agent"], &["RUN=marker"]);

        assert_eq!(variables(&buffer), ["RUN=marker"]);
    }

    #[test]
    fn the_caller_is_never_found() {
        let own = std::process::id();
        let entry = format!("PATH={}", std::env::var("PATH").unwrap_or_default());
        let mut scanner = Scanner::new().unwrap();

        assert!(list().unwrap().contains(&own.cast_signed()));
        assert!(!scanner.find(entry.as_bytes()).unwrap().contains(&own));
    }
}
