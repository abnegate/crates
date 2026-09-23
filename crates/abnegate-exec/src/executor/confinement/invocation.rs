use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::path::PathBuf;

use tokio::process::Child;
use tokio::process::Command;

use crate::executor::session;

/// The bubblewrap option that reads further NUL-separated arguments from a
/// file descriptor.
const DESCRIPTOR_FLAG: &str = "--args";

/// How many of [`Invocation::arguments`] precede `--args <descriptor>`.
/// Bubblewrap applies its options in order, so the `--setenv`s read from the
/// descriptor must follow the `--clearenv` its arguments open with.
const DESCRIPTOR_POSITION: usize = 1;

/// A backend executable and the argument vector that runs a command inside it.
///
/// Run it with [`Invocation::spawn`], which starts the backend with exactly
/// [`Invocation::environment`] and hands it
/// [`Invocation::descriptor_arguments`] through an inherited pipe.
///
/// No environment value ever appears in [`Invocation::arguments`], where any
/// user on the host can read it. Seatbelt receives the command's environment
/// as its own: `sandbox-exec` is a platform binary under System Integrity
/// Protection, so the loader ignores `DYLD_*` for it. Bubblewrap is
/// dynamically linked and, on most distributions, not setuid, so glibc would
/// honour `LD_PRELOAD` and its kin in the bubblewrap process itself, before any
/// namespace exists; it therefore starts with an empty environment and sets
/// the command's from the descriptor.
///
/// A caller that runs [`Invocation::program`] with [`Invocation::arguments`]
/// itself must clear its own environment and set exactly
/// [`Invocation::environment`], or whatever it holds reaches the backend and,
/// under seatbelt, the command. Bubblewrap's arguments open with `--clearenv`,
/// so under bubblewrap a caller that forgets still hands the command nothing
/// of its own, though the command then also lacks the environment it asked
/// for, which only the descriptor carries.
///
/// `Debug` prints environment names and never their values.
#[derive(Clone, PartialEq, Eq)]
pub struct Invocation {
    program: PathBuf,
    arguments: Vec<String>,
    environment: BTreeMap<String, String>,
    descriptor_arguments: Vec<String>,
}

impl Invocation {
    pub(super) fn new(
        program: PathBuf,
        arguments: Vec<String>,
        environment: BTreeMap<String, String>,
        descriptor_arguments: Vec<String>,
    ) -> Self {
        Self {
            program,
            arguments,
            environment,
            descriptor_arguments,
        }
    }

    /// The backend executable.
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// The backend's arguments, ending with the command and the command's
    /// arguments.
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    /// The environment of the backend process itself, and nothing inherited.
    pub fn environment(&self) -> &BTreeMap<String, String> {
        &self.environment
    }

    /// Arguments the backend reads from an inherited descriptor, because they
    /// carry the command's environment. [`Invocation::spawn`] names that
    /// descriptor with `--args <descriptor>` after the first of
    /// [`Invocation::arguments`]. Empty for a backend that takes the
    /// environment as its own.
    pub fn descriptor_arguments(&self) -> &[String] {
        &self.descriptor_arguments
    }

    /// Spawn the backend in a new session with exactly
    /// [`Invocation::environment`], once `configure` has set its working
    /// directory and standard streams, and feed it
    /// [`Invocation::descriptor_arguments`] through a pipe.
    ///
    /// # Panics
    ///
    /// When called outside a Tokio runtime.
    pub fn spawn(&self, configure: impl FnOnce(&mut Command)) -> io::Result<Child> {
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
        let (leading, trailing) = self
            .arguments
            .split_at(DESCRIPTOR_POSITION.min(self.arguments.len()));
        command
            .args(leading)
            .arg(DESCRIPTOR_FLAG)
            .arg(reader.as_raw_fd().to_string())
            .args(trailing);
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
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;

    use crate::executor::Backend;
    use crate::executor::Confinement;
    use crate::protocol::ConfinementRequest;

    use super::*;

    /// What a stand-in backend was started with.
    struct Started {
        arguments: Vec<String>,
        descriptor: String,
        environment: Vec<String>,
    }

    /// Spawn `invocation` with its program swapped for a script that records
    /// its arguments, whatever `--args <descriptor>` names, and its
    /// environment, less the variables the shell sets for itself.
    async fn started(invocation: Invocation) -> Started {
        const SHELL_OWN: [&str; 4] = ["PWD=", "OLDPWD=", "SHLVL=", "_="];
        let directory = tempfile::tempdir().unwrap();
        let backend = directory.path().join("backend");
        std::fs::write(
            &backend,
            "#!/bin/sh\n\
             printf '%s\\n' \"$@\" > \"$0.arguments\"\n\
             [ \"$2\" = --args ] && cat \"/dev/fd/$3\" > \"$0.descriptor\"\n\
             /usr/bin/env > \"$0.environment\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&backend, std::fs::Permissions::from_mode(0o755)).unwrap();
        let recorded = |suffix: &str| {
            std::fs::read_to_string(directory.path().join(format!("backend.{suffix}")))
                .unwrap_or_default()
        };

        let status = Invocation {
            program: backend.clone(),
            ..invocation
        }
        .spawn(|_| {})
        .unwrap()
        .wait()
        .await
        .unwrap();

        assert!(status.success(), "{status:?}");
        Started {
            arguments: recorded("arguments").lines().map(str::to_string).collect(),
            descriptor: recorded("descriptor"),
            environment: recorded("environment")
                .lines()
                .filter(|line| !SHELL_OWN.iter().any(|name| line.starts_with(name)))
                .map(str::to_string)
                .collect(),
        }
    }

    #[test]
    fn debug_names_the_environment_without_its_values() {
        let invocation = Invocation::new(
            PathBuf::from("/usr/bin/sandbox-exec"),
            vec!["--".to_string()],
            BTreeMap::from([("APP_MASTER_KEY".to_string(), "hunter2".to_string())]),
            vec![
                "--setenv".to_string(),
                "APP_MASTER_KEY".to_string(),
                "hunter2".to_string(),
            ],
        );

        let debug = format!("{invocation:?}");

        assert!(debug.contains("APP_MASTER_KEY"), "{debug}");
        assert!(!debug.contains("hunter2"), "{debug}");
    }

    /// Bubblewrap applies `--clearenv` where it meets it, so the descriptor
    /// that carries the `--setenv`s is read after it, and the backend itself
    /// starts with none of the caller's environment.
    #[tokio::test]
    async fn spawning_bubblewrap_clears_the_environment_then_reads_the_descriptor() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let invocation = Confinement::new("/bin/cat", vec![], &root)
            .with_roots(&ConfinementRequest {
                read_roots: vec![root.clone()],
                write_roots: vec![root.clone()],
                process_tree: None,
            })
            .with_environment(HashMap::from([(
                "APP_MASTER_KEY".to_string(),
                "hunter2".to_string(),
            )]))
            .invocation(Some(Backend::Bubblewrap))
            .unwrap();

        let started = started(invocation.clone()).await;

        assert_eq!(started.arguments[..2], ["--clearenv", "--args"]);
        assert!(
            started.arguments[2].parse::<u32>().is_ok(),
            "{:?}",
            started.arguments
        );
        assert_eq!(started.arguments[3..], invocation.arguments[1..]);
        assert!(
            started.environment.is_empty(),
            "bubblewrap inherited {:?}",
            started.environment
        );
        assert!(
            started
                .descriptor
                .contains("--setenv\0APP_MASTER_KEY\0hunter2\0"),
            "{:?}",
            started.descriptor
        );
        assert!(!started.descriptor.contains("--clearenv"));
        assert!(
            started
                .arguments
                .iter()
                .all(|argument| !argument.contains("hunter2"))
        );
    }

    #[tokio::test]
    async fn spawning_without_descriptor_arguments_passes_only_the_backends_environment() {
        let invocation = Invocation::new(
            PathBuf::new(),
            vec!["-p".to_string(), "(version 1)".to_string()],
            BTreeMap::from([("REQUESTED".to_string(), "value".to_string())]),
            Vec::new(),
        );

        let started = started(invocation).await;

        assert_eq!(started.arguments, ["-p", "(version 1)"]);
        assert_eq!(started.descriptor, "");
        assert_eq!(started.environment, ["REQUESTED=value"]);
    }
}
