//! OS-level confinement for spawned commands.
//!
//! A confined command runs under the host sandbox backend — seatbelt on macOS,
//! bubblewrap on Linux — with an explicit set of readable and writable roots and
//! no network access at all.
//!
//! There are two shapes of confined job. [`ConfinementMode::SingleCommand`] runs
//! exactly one executable, which may neither fork nor exec: right for a
//! verification recipe, wrong for a build tool.
//! [`ConfinementMode::ProcessTree`] lets the command fork and exec, bounded by
//! an explicit set of executable directories, so `cargo test` can reach `rustc`,
//! a linker and the test binaries it just built without the sandbox admitting
//! anything else.
//!
//! Confinement is never assumed to work. [`Confinement::probe`] executes real
//! commands inside the sandbox and asserts that a denied file stays unreadable
//! and that a connection to a live local listener never arrives. Tree mode is
//! probed separately and more strictly: it forks before reaching for the
//! network, so the denial is proven for a descendant rather than for the one
//! process the sandbox was applied to. A host that cannot prove those
//! properties refuses to run confined jobs rather than running them unconfined.

mod backend;
mod bubblewrap;
mod environment;
mod error;
mod invocation;
mod mode;
mod path;
mod probe;
mod resolved;
mod seatbelt;
mod workspace;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use crate::protocol::ConfinementRequest;

pub use backend::Backend;
pub use backend::HOST_BACKEND;
pub use error::ConfinementError;
pub use invocation::Invocation;
pub use mode::ConfinementMode;

use backend::backend_executable;
use environment::complete_environment;
use environment::resolve_command;
use path::canonical;
use path::canonical_roots;
use probe::probe_process_tree;
use probe::probe_single_command;
use resolved::Resolved;
use resolved::resolve_execute_roots;

/// A command together with the filesystem it is allowed to see.
///
/// `Debug` prints the names in the environment and never their values.
#[derive(Clone)]
pub struct Confinement {
    command: String,
    arguments: Vec<String>,
    working_dir: PathBuf,
    read_roots: Vec<PathBuf>,
    write_roots: Vec<PathBuf>,
    execute_roots: Vec<PathBuf>,
    mode: ConfinementMode,
    environment: BTreeMap<String, String>,
}

impl Confinement {
    pub fn new(
        command: impl Into<String>,
        arguments: Vec<String>,
        working_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            command: command.into(),
            arguments,
            working_dir: working_dir.into(),
            read_roots: Vec::new(),
            write_roots: Vec::new(),
            execute_roots: Vec::new(),
            mode: ConfinementMode::SingleCommand,
            environment: BTreeMap::new(),
        }
    }

    /// Grant the roots named by a protocol request.
    pub fn with_roots(mut self, request: &ConfinementRequest) -> Self {
        self.read_roots = request.read_roots.clone();
        self.write_roots = request.write_roots.clone();
        self.mode = request.mode();
        self.execute_roots = request
            .process_tree
            .as_ref()
            .map(|tree| tree.execute_roots.clone())
            .unwrap_or_default();
        self
    }

    /// How much of a process tree this confinement admits.
    pub fn mode(&self) -> ConfinementMode {
        self.mode
    }

    /// Set the environment handed to the confined command.
    pub fn with_environment(mut self, environment: HashMap<String, String>) -> Self {
        self.environment = environment.into_iter().collect();
        self
    }

    /// Whether this host has a confinement backend installed. Availability is
    /// not proof: [`Confinement::probe`] still has to pass before a confined
    /// job may run.
    pub fn is_available() -> bool {
        HOST_BACKEND.is_some_and(|backend| backend_executable(backend).is_ok())
    }

    /// Translate into a backend invocation. `None` means the host has no
    /// backend, which is an error rather than an unconfined spawn.
    pub fn invocation(&self, backend: Option<Backend>) -> Result<Invocation, ConfinementError> {
        let backend = backend.ok_or(ConfinementError::UnsupportedPlatform)?;
        let resolved = self.resolve()?;
        match backend {
            Backend::Seatbelt => Ok(Invocation {
                program: PathBuf::from(backend.executable()),
                arguments: seatbelt::arguments(&resolved)?,
                environment: resolved.environment,
            }),
            Backend::Bubblewrap => Ok(Invocation {
                program: PathBuf::from(backend.executable()),
                arguments: bubblewrap::arguments(&resolved)?,
                environment: BTreeMap::new(),
            }),
        }
    }

    /// Translate into an invocation for the backend of the running host.
    pub fn host_invocation(&self) -> Result<Invocation, ConfinementError> {
        self.invocation(HOST_BACKEND)
    }

    /// Prove that this host's confinement actually confines, caching the
    /// verdict per mode for the lifetime of the process.
    ///
    /// A tree is a strictly larger claim than a single command, so its verdict
    /// is cached separately: a host that can prove one is not thereby taken to
    /// have proven the other.
    pub async fn probe(mode: ConfinementMode) -> Result<(), ConfinementError> {
        match mode {
            ConfinementMode::SingleCommand => probe_single_command().await,
            ConfinementMode::ProcessTree => probe_process_tree().await,
        }
    }

    fn resolve(&self) -> Result<Resolved, ConfinementError> {
        let command = resolve_command(&self.command, &self.environment)?;
        let working_dir = canonical(&self.working_dir)?;
        let read_roots = canonical_roots(&self.read_roots)?;
        let write_roots = canonical_roots(&self.write_roots)?;
        let execute_roots = resolve_execute_roots(self.mode, &self.execute_roots)?;
        let environment = complete_environment(
            &self.environment,
            &command,
            write_roots.first().unwrap_or(&working_dir),
        );

        Ok(Resolved {
            command,
            arguments: self.arguments.clone(),
            working_dir,
            read_roots,
            write_roots,
            execute_roots,
            mode: self.mode,
            environment,
        })
    }
}

impl fmt::Debug for Confinement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Confinement")
            .field("command", &self.command)
            .field("arguments", &self.arguments)
            .field("working_dir", &self.working_dir)
            .field("read_roots", &self.read_roots)
            .field("write_roots", &self.write_roots)
            .field("execute_roots", &self.execute_roots)
            .field("mode", &self.mode)
            .field("environment", &self.environment.keys())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_names_the_environment_without_its_values() {
        let confinement = Confinement::new("/bin/cat", vec![], "/tmp").with_environment(
            HashMap::from([("APP_MASTER_KEY".to_string(), "hunter2".to_string())]),
        );

        let debug = format!("{confinement:?}");

        assert!(debug.contains("APP_MASTER_KEY"), "{debug}");
        assert!(!debug.contains("hunter2"), "{debug}");
    }

    #[test]
    fn test_unsupported_platform_never_produces_an_invocation() {
        let confinement = Confinement::new("/bin/cat", vec![], "/tmp");
        assert_eq!(
            confinement.invocation(None),
            Err(ConfinementError::UnsupportedPlatform)
        );
    }
}
