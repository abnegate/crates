use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::path::PathBuf;

use crate::login::LoginError;
use crate::login::codex::Device;
use crate::login::codex::HOME;
use crate::login::codex::LOGOUT;
use crate::login::codex::Limits;
use crate::login::codex::saved;
use crate::login::output::Output;
use crate::login::process::Process;

/// Codex's own device-code sign-in, `codex login --device-auth`, run with
/// exactly the environment it is given.
///
/// `Debug` names the variables in the environment and shows none of their
/// values.
#[derive(Clone)]
pub struct DeviceLogin {
    executable: PathBuf,
    environment: BTreeMap<String, String>,
    limits: Limits,
}

impl DeviceLogin {
    /// A sign-in run by the codex CLI at `executable`, with an empty
    /// environment and the default [`Limits`].
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            environment: BTreeMap::new(),
            limits: Limits::default(),
        }
    }

    /// This sign-in, giving codex exactly `environment` and nothing of this
    /// process's own, apart from the `CODEX_HOME` each step sets.
    pub fn with_environment(mut self, environment: BTreeMap<String, String>) -> Self {
        self.environment = environment;
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
    /// promotes the new one. One device sign-in may run per root at a time,
    /// as each one clears what earlier attempts left there.
    ///
    /// # Errors
    ///
    /// [`LoginError::Unavailable`] when codex cannot be started,
    /// [`LoginError::Failed`] in codex's own words when it exits first,
    /// [`LoginError::Unreadable`] when it prints no whole prompt within
    /// [`Limits::prompt`], and [`LoginError::Filesystem`] when the staging
    /// directory cannot be prepared. Codex is stopped and the staging
    /// directory removed on every one.
    pub async fn start(&self, root: &Path) -> Result<Device, LoginError> {
        Device::start(self, root).await
    }

    /// Runs `codex logout` against `home`, which revokes the login upstream
    /// and deletes it.
    ///
    /// # Errors
    ///
    /// [`LoginError::Unavailable`] when codex cannot be started,
    /// [`LoginError::Failed`] in codex's own words when it refuses, and
    /// [`LoginError::Unreadable`] when it does not finish in time.
    pub async fn logout(&self, home: &Path) -> Result<(), LoginError> {
        let mut command = Process::command(&self.executable, LOGOUT, &self.environment);
        command.env(HOME, home);
        let output = Output::capture(command, &self.executable).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(output.failed(&self.executable))
        }
    }

    /// Whether `home` holds a codex login: its `auth.json` is a regular file,
    /// not a link or a directory. Codex never checks the credentials in it
    /// until a turn needs them.
    pub fn signed_in(home: &Path) -> bool {
        saved(home)
    }

    pub(crate) fn executable(&self) -> &Path {
        &self.executable
    }

    pub(crate) fn environment(&self) -> &BTreeMap<String, String> {
        &self.environment
    }

    pub(crate) fn limits(&self) -> Limits {
        self.limits
    }
}

impl fmt::Debug for DeviceLogin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceLogin")
            .field("executable", &self.executable)
            .field("environment", &self.environment.keys().collect::<Vec<_>>())
            .field("limits", &self.limits)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_names_the_environment_without_its_values() {
        let login = DeviceLogin::new("/usr/local/bin/codex").with_environment(BTreeMap::from([(
            "HTTPS_PROXY".to_string(),
            "http://user:hunter2@proxy.example:8080".to_string(),
        )]));

        let rendered = format!("{login:?}");

        assert!(rendered.contains("HTTPS_PROXY"), "{rendered}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
    }
}
