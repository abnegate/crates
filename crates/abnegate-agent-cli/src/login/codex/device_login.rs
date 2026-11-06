use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use abnegate_secret::SecretValue;

use crate::login::Cli;
use crate::login::LoginError;
use crate::login::codex::Device;
use crate::login::codex::HOME;
use crate::login::codex::LOGOUT;
use crate::login::codex::Limits;
use crate::login::output::Output;
use crate::login::process::Process;

/// Codex's own device-code sign-in, `codex login --device-auth`, run with
/// exactly the environment it is given.
///
/// `Debug` names the variables in the environment and shows none of their
/// values.
#[derive(Debug, Clone)]
pub struct DeviceLogin {
    cli: Cli,
    limits: Limits,
}

impl DeviceLogin {
    /// A sign-in run by the codex CLI at `executable`, with an empty
    /// environment and the default [`Limits`].
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self::from(Cli::new(executable))
    }

    /// This sign-in, giving codex exactly `environment`, its secrets and
    /// nothing of this process's own, apart from the `CODEX_HOME` each step
    /// sets. It replaces the variables given before and keeps the secrets.
    pub fn with_environment(mut self, environment: BTreeMap<String, String>) -> Self {
        self.cli = self.cli.with_environment(environment);
        self
    }

    /// This sign-in, also giving codex `variable` set to the secret `value`,
    /// such as an `HTTPS_PROXY` that carries a password. It wins over a
    /// variable of the same name, and no error repeats it.
    pub fn with_secret(
        mut self,
        variable: impl Into<String>,
        value: impl Into<SecretValue>,
    ) -> Self {
        self.cli = self.cli.with_secret(variable, value);
        self
    }

    /// This sign-in, held to `limits`.
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Starts `codex login --device-auth` for the logins kept under `root`,
    /// which the caller has created, and returns once codex has printed the
    /// link and code a person must enter.
    ///
    /// Codex runs in its own process group with nothing on its stdin, its
    /// `CODEX_HOME` a private [`Staging`](crate::login::codex::Staging)
    /// directory under `<root>/.login`, because it deletes any login in its
    /// home before it prints a code: no login is touched until the caller
    /// promotes the new one. Sign-ins under one root may overlap: each one
    /// clears only what earlier attempts left there once nothing holds them,
    /// never a running attempt or a [`Staging`](crate::login::codex::Staging)
    /// that has not been dropped.
    ///
    /// `root` must be writable by this user alone, as every directory above
    /// it must be: whoever else can write to it can swap `.login` for a link
    /// between its removal and its creation, and read the login codex saves.
    ///
    /// Dropping the future before it resolves stops codex, its whole group
    /// included, and removes the staging directory.
    ///
    /// # Errors
    ///
    /// [`LoginError::Unavailable`] when codex cannot be started,
    /// [`LoginError::Failed`] in codex's own words when it exits first,
    /// [`LoginError::Unreadable`] when it prints no whole prompt within
    /// [`Limits::prompt`], and [`LoginError::Filesystem`] when the staging
    /// directory cannot be prepared. Codex is stopped and the staging
    /// directory removed on every one, and no reason repeats a secret.
    pub async fn start(&self, root: &Path) -> Result<Device, LoginError> {
        Device::start(self, root)
            .await
            .map_err(|error| self.cli.scrubbed(error))
    }

    /// Runs `codex logout` against `home`, which revokes the login upstream
    /// and deletes it, giving it [`Limits::command`] to finish.
    ///
    /// # Errors
    ///
    /// [`LoginError::Unavailable`] when codex cannot be started,
    /// [`LoginError::Failed`] in codex's own words when it refuses, and
    /// [`LoginError::Unreadable`] when it does not finish in time. No reason
    /// repeats a secret.
    pub async fn logout(&self, home: &Path) -> Result<(), LoginError> {
        let executable = self.cli.executable();
        let mut command = Process::command(&self.cli, LOGOUT);
        command.env(HOME, home);
        let output = Output::capture(command, executable, self.limits.command)
            .await
            .map_err(|error| self.cli.scrubbed(error))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(self.cli.scrubbed(output.failed(&self.cli)))
        }
    }

    pub(crate) fn cli(&self) -> &Cli {
        &self.cli
    }

    pub(crate) fn limits(&self) -> Limits {
        self.limits
    }
}

/// A sign-in run by `cli`, with the default [`Limits`].
impl From<Cli> for DeviceLogin {
    fn from(cli: Cli) -> Self {
        Self {
            cli,
            limits: Limits::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_names_the_environment_without_its_values() {
        let login = DeviceLogin::new("/usr/local/bin/codex")
            .with_environment(BTreeMap::from([(
                "NO_PROXY".to_string(),
                "internal.example".to_string(),
            )]))
            .with_secret("HTTPS_PROXY", "http://user:hunter2@proxy.example:8080");

        let rendered = format!("{login:?}");

        assert!(rendered.contains("HTTPS_PROXY"), "{rendered}");
        assert!(rendered.contains("NO_PROXY"), "{rendered}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(!rendered.contains("internal.example"), "{rendered}");
    }
}
