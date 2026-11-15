use std::io;
use std::ops::Deref;
use std::ops::DerefMut;

use tokio::process::Child;

use crate::tool::Network;

/// A command built to reach only `network`, held with it so a failure to
/// start is always reported for the network the command was built with.
pub(crate) struct Command {
    inner: tokio::process::Command,
    network: Network,
}

impl Command {
    pub(super) fn new(inner: tokio::process::Command, network: Network) -> Self {
        Self { inner, network }
    }

    /// A command that reaches whatever network the host can, for a test that
    /// builds its own.
    #[cfg(test)]
    pub(crate) fn open(inner: tokio::process::Command) -> Self {
        Self::new(inner, Network::Open)
    }

    /// Start the command, with the reason a denied one most likely failed to
    /// start in its error, as [`Network::spawn_error`] puts it.
    pub(crate) fn spawn(&mut self) -> io::Result<Child> {
        self.inner
            .spawn()
            .map_err(|error| self.network.spawn_error(error))
    }
}

impl Deref for Command {
    type Target = tokio::process::Command;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Command {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
