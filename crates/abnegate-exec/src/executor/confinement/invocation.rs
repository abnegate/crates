use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::fd::OwnedFd;
use std::path::PathBuf;

use tokio::process::Child;
use tokio::process::Command;

use crate::executor::session;

/// The bubblewrap option that reads further NUL-separated arguments from a
/// file descriptor.
const DESCRIPTOR_FLAG: &str = "--args";

/// A backend executable and the argument vector that runs a command inside it.
///
/// No environment value ever appears in `arguments`, where any user on the
/// host can read it. Seatbelt receives the command's environment as its own:
/// `sandbox-exec` is a platform binary under System Integrity Protection, so
/// the loader ignores `DYLD_*` for it. Bubblewrap is dynamically linked and,
/// on most distributions, not setuid, so glibc would honour `LD_PRELOAD` and
/// its kin in the bubblewrap process itself, before any namespace exists; it
/// therefore starts with an empty environment and sets the command's from
/// `descriptor_arguments`.
///
/// `Debug` prints environment names and never their values.
#[derive(Clone, PartialEq, Eq)]
pub struct Invocation {
    /// The backend executable
    pub program: PathBuf,
    /// Its arguments, ending with the command and the command's arguments
    pub arguments: Vec<String>,
    /// The environment of the backend process itself, and nothing inherited
    pub environment: BTreeMap<String, String>,
    /// Arguments the backend reads from an inherited descriptor, named by
    /// `--args <descriptor>` ahead of `arguments`, because they carry the
    /// command's environment. Empty for a backend that takes the environment
    /// as its own.
    pub descriptor_arguments: Vec<String>,
}

impl Invocation {
    /// Spawn the backend in a new session with exactly
    /// [`Invocation::environment`], once `configure` has set its working
    /// directory and standard streams.
    pub(crate) fn spawn(&self, configure: impl FnOnce(&mut Command)) -> io::Result<Child> {
        let mut command = Command::new(&self.program);
        command.env_clear().envs(&self.environment);
        configure(&mut command);
        if self.descriptor_arguments.is_empty() {
            command.args(&self.arguments);
            session::lead(&mut command, None);
            return command.spawn();
        }

        let (reader, mut writer) = io::pipe()?;
        let reader = OwnedFd::from(reader);
        command
            .arg(DESCRIPTOR_FLAG)
            .arg(reader.as_raw_fd().to_string())
            .args(&self.arguments);
        session::lead(&mut command, Some(reader));
        let child = command.spawn()?;
        drop(command);
        let payload = self.descriptor_payload();
        tokio::task::spawn_blocking(move || writer.write_all(&payload));
        Ok(child)
    }

    fn descriptor_payload(&self) -> Vec<u8> {
        self.descriptor_arguments
            .iter()
            .flat_map(|argument| argument.bytes().chain([0]))
            .collect()
    }
}

impl fmt::Debug for Invocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Invocation")
            .field("program", &self.program)
            .field("arguments", &self.arguments)
            .field("environment", &self.environment.keys())
            .field(
                "descriptor_arguments",
                &format_args!("<{} arguments>", self.descriptor_arguments.len()),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;

    use super::*;

    #[test]
    fn debug_names_the_environment_without_its_values() {
        let invocation = Invocation {
            program: PathBuf::from("/usr/bin/sandbox-exec"),
            arguments: vec!["--".to_string()],
            environment: BTreeMap::from([("APP_MASTER_KEY".to_string(), "hunter2".to_string())]),
            descriptor_arguments: vec![
                "--setenv".to_string(),
                "APP_MASTER_KEY".to_string(),
                "hunter2".to_string(),
            ],
        };

        let debug = format!("{invocation:?}");

        assert!(debug.contains("APP_MASTER_KEY"), "{debug}");
        assert!(!debug.contains("hunter2"), "{debug}");
    }

    /// A stand-in backend that prints whatever `--args <descriptor>` names.
    #[tokio::test]
    async fn descriptor_arguments_reach_the_child_through_the_named_descriptor() {
        let directory = tempfile::tempdir().unwrap();
        let backend = directory.path().join("backend");
        std::fs::write(
            &backend,
            "#!/bin/sh\n[ \"$1\" = --args ] || exit 64\ncat \"/dev/fd/$2\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&backend, std::fs::Permissions::from_mode(0o755)).unwrap();
        let invocation = Invocation {
            program: backend,
            arguments: Vec::new(),
            environment: BTreeMap::new(),
            descriptor_arguments: vec!["--setenv".to_string(), "hunter2".to_string()],
        };

        let output = invocation
            .spawn(|command| {
                command.stdout(Stdio::piped());
            })
            .unwrap()
            .wait_with_output()
            .await
            .unwrap();

        assert_eq!(output.status.code(), Some(0));
        assert_eq!(output.stdout, b"--setenv\0hunter2\0");
    }
}
