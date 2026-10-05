use std::io;

use nix::sys::signal::Signal;

/// Finds nothing: this host offers no way to read another process's
/// environment that the sweep knows.
pub(super) struct Scanner;

impl Scanner {
    pub(super) fn new() -> io::Result<Self> {
        Ok(Self)
    }

    pub(super) fn find(&mut self, _entry: &[u8]) -> io::Result<Vec<u32>> {
        Ok(Vec::new())
    }

    pub(super) fn deliver(
        &mut self,
        _pid: u32,
        _entry: &[u8],
        _signal: Signal,
    ) -> io::Result<bool> {
        Ok(false)
    }
}
