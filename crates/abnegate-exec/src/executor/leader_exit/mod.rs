//! Learning that a child has exited without reaping it.
//!
//! An unreaped child is a zombie that keeps its pid, and so its process
//! group's id, from being reused: signalling the group stays safe until the
//! child is waited on. Each `SIGCHLD` prompts a check that leaves the child in
//! place -- `waitid` with `WNOWAIT` on Linux, a `NOTE_EXIT` kqueue event on
//! macOS and FreeBSD.

#[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
mod kqueue;
#[cfg(any(target_os = "linux", target_os = "android"))]
mod waitid;

#[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
pub(super) use kqueue::LeaderExit;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) use waitid::LeaderExit;
