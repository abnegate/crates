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

use crate::protocol::{ConfinementRequest, ProcessTreeRequest};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::OnceCell;
use tokio::time::timeout;

const SEATBELT: &str = "/usr/bin/sandbox-exec";
const BUBBLEWRAP: &str = "/usr/bin/bwrap";

const SEATBELT_READ_TREES: [&str; 4] = ["/System", "/dev", "/usr/lib", "/usr/share"];

/// `/bin/sh` resolves which shell binary to become by reading this directory,
/// so a tree driven through a shell cannot start without it. Single-command
/// mode does not need it because nothing re-execs.
const SEATBELT_TREE_READ_TREES: [&str; 1] = ["/private/var/select"];

/// Name resolution and account enumeration stay denied even though `system.sb`
/// grants a broad read of the system volume.
const SEATBELT_DENIED_FILES: [&str; 2] = ["/private/etc/hosts", "/private/etc/passwd"];

const BUBBLEWRAP_READ_TREES: [&str; 4] = ["/usr", "/bin", "/lib", "/lib64"];
const BUBBLEWRAP_LOADER_FILES: [&str; 3] =
    ["/etc/ld.so.cache", "/etc/ld.so.conf", "/etc/localtime"];

const HOME: &str = "HOME";
const TMPDIR: &str = "TMPDIR";
const TMP: &str = "TMP";
const TEMP: &str = "TEMP";
const PATH: &str = "PATH";
const LANG: &str = "LANG";
const LC_ALL: &str = "LC_ALL";

const SYSTEM_PATH: &str = "/usr/bin:/bin";
const LOCALE: &str = "C.UTF-8";

const PROBE_COMMAND: &str = "/bin/cat";
const PROBE_ALLOWED: &str = "allowed";
const PROBE_DENIED: &str = "denied";
const PROBE_ALLOWED_CONTENT: &str = "allowed\n";
const PROBE_DENIED_CONTENT: &str = "secret\n";
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const PROBE_SETTLE: Duration = Duration::from_millis(25);

/// The shell the tree probe uses to build a real parent/child chain.
const PROBE_SHELL: &str = "/bin/sh";

const PROBE_PARENT_SCRIPT: &str = "parent.sh";
const PROBE_CHILD_SCRIPT: &str = "child.sh";
const PROBE_EXECUTE_SCRIPT: &str = "execute.sh";
const PROBE_PARENT_IDENTIFIER: &str = "parent.pid";
const PROBE_CHILD_IDENTIFIER: &str = "child.pid";
const PROBE_NETWORK_STATUS: &str = "network.status";
const PROBE_EXECUTE_STATUS: &str = "execute.status";
/// The planted executable is a script, not a copy of a system binary: macOS
/// kills a copied system binary on sight for its lost code signature, which
/// would make the execute-root check pass without the sandbox doing anything.
const PROBE_PLANTED_COMMAND: &str = "planted.sh";
const PROBE_PLANTED_MODE: u32 = 0o755;

/// Commands able to open a TCP connection, in preference order. The probe needs
/// exactly one of them to exist; a host with none cannot prove its sandbox.
const NETWORK_PROBE_COMMANDS: [&str; 5] = [
    "/usr/bin/nc",
    "/bin/nc",
    "/usr/bin/ncat",
    "/usr/bin/curl",
    "/bin/curl",
];

/// Errors raised while preparing or proving confinement.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ConfinementError {
    #[error("Confinement is not supported on this platform")]
    UnsupportedPlatform,

    #[error("Confinement backend is unusable: {path}: {reason}")]
    BackendUnusable { path: String, reason: String },

    #[error("Confined path must be absolute: {0}")]
    RelativePath(String),

    #[error("Confined path is not valid UTF-8: {0}")]
    NonUnicodePath(String),

    #[error("Confined path contains a control character: {0}")]
    ControlCharacterInPath(String),

    #[error("Confined path is unusable: {path}: {reason}")]
    UnusablePath { path: String, reason: String },

    #[error("Confined command not found: {0}")]
    CommandNotFound(String),

    #[error("A confined process tree needs at least one execute root")]
    ProcessTreeWithoutExecuteRoots,

    #[error("Execute root would admit every executable on the host: {0}")]
    UnboundedExecuteRoot(String),

    #[error("Confinement could not be proven: {0}")]
    Unproven(String),
}

/// How much of a process tree a confined job may create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfinementMode {
    /// One executable runs and nothing else. Forking and further execs are
    /// denied outright.
    SingleCommand,
    /// The command may fork and exec, bounded by an explicit set of executable
    /// directories.
    ProcessTree,
}

/// The OS mechanism used to confine a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Seatbelt,
    Bubblewrap,
}

#[cfg(target_os = "macos")]
pub const HOST_BACKEND: Option<Backend> = Some(Backend::Seatbelt);

#[cfg(target_os = "linux")]
pub const HOST_BACKEND: Option<Backend> = Some(Backend::Bubblewrap);

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub const HOST_BACKEND: Option<Backend> = None;

impl Backend {
    /// Absolute path of the backend executable.
    pub const fn executable(self) -> &'static str {
        match self {
            Backend::Seatbelt => SEATBELT,
            Backend::Bubblewrap => BUBBLEWRAP,
        }
    }

    /// A file the sandbox must refuse to read, used by the probe.
    const fn denied_system_file(self) -> &'static str {
        match self {
            Backend::Seatbelt => SEATBELT_DENIED_FILES[0],
            Backend::Bubblewrap => "/etc/hosts",
        }
    }

    /// Whether the backend can refuse an exec of a file the tree can otherwise
    /// see.
    ///
    /// Seatbelt filters `process-exec` by path, so a binary dropped into a
    /// writable root stays unrunnable. Bubblewrap has no exec filter: its bound
    /// is the mount namespace, where an executable outside every bind does not
    /// exist at all. Both bound the executable set; only seatbelt can be asked
    /// to prove it against a file that is present.
    pub const fn enforces_execute_roots(self) -> bool {
        match self {
            Backend::Seatbelt => true,
            Backend::Bubblewrap => false,
        }
    }

    /// Whether the backend can refuse the command a second process at all.
    ///
    /// Seatbelt filters `process-fork`, so single-command mode really is one
    /// process. Bubblewrap has no such primitive: it bounds a sandbox by its
    /// namespaces, so a fork succeeds and the child lands inside the same mount,
    /// network and PID namespaces. Containment is identical either way -- the
    /// child sees the same filesystem, reaches no network, and dies with the
    /// sandbox through `--die-with-parent` and PID namespace teardown -- but
    /// only seatbelt can be asked to prevent the second process existing.
    pub const fn enforces_single_process(self) -> bool {
        match self {
            Backend::Seatbelt => true,
            Backend::Bubblewrap => false,
        }
    }
}

/// A backend executable and the argument vector that runs a command inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    /// Environment for the backend process itself. Bubblewrap clears its own
    /// environment and carries the command's through `--setenv`, so this is
    /// empty there and complete under seatbelt.
    pub environment: BTreeMap<String, String>,
}

/// A command together with the filesystem it is allowed to see.
#[derive(Debug, Clone)]
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

struct Resolved {
    command: PathBuf,
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
                arguments: seatbelt_arguments(&resolved)?,
                environment: resolved.environment,
            }),
            Backend::Bubblewrap => Ok(Invocation {
                program: PathBuf::from(backend.executable()),
                arguments: bubblewrap_arguments(&resolved)?,
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

/// Canonicalise and bound the executable directories a tree may launch from.
///
/// Single-command mode has no execute roots by construction. A tree needs at
/// least one, and none of them may be the filesystem root: an execute root of
/// `/` is "allow every exec on the host", which is the thing this mode exists
/// to avoid.
fn resolve_execute_roots(
    mode: ConfinementMode,
    roots: &[PathBuf],
) -> Result<Vec<PathBuf>, ConfinementError> {
    if mode == ConfinementMode::SingleCommand {
        return Ok(Vec::new());
    }
    if roots.is_empty() {
        return Err(ConfinementError::ProcessTreeWithoutExecuteRoots);
    }

    let execute_roots = canonical_roots(roots)?;
    for root in &execute_roots {
        if root.parent().is_none() {
            return Err(ConfinementError::UnboundedExecuteRoot(
                root.display().to_string(),
            ));
        }
    }
    Ok(execute_roots)
}

fn seatbelt_arguments(resolved: &Resolved) -> Result<Vec<String>, ConfinementError> {
    let command = text(&resolved.command)?;
    let mut arguments = vec![
        "-p".to_string(),
        seatbelt_profile(resolved)?,
        "--".to_string(),
        command.to_string(),
    ];
    arguments.extend(resolved.arguments.iter().cloned());
    Ok(arguments)
}

fn seatbelt_profile(resolved: &Resolved) -> Result<String, ConfinementError> {
    let command = text(&resolved.command)?;
    let tree = resolved.mode == ConfinementMode::ProcessTree;

    let mut trees: BTreeSet<&str> = SEATBELT_READ_TREES.into_iter().collect();
    for root in resolved.read_roots.iter().chain(&resolved.write_roots) {
        trees.insert(text(root)?);
    }
    if tree {
        trees.extend(SEATBELT_TREE_READ_TREES);
        for root in &resolved.execute_roots {
            trees.insert(text(root)?);
        }
    }

    let mut metadata: BTreeSet<&Path> = BTreeSet::new();
    for path in trees
        .iter()
        .map(Path::new)
        .chain([resolved.command.as_path()])
    {
        metadata.extend(
            path.ancestors()
                .filter(|ancestor| ancestor.parent().is_some()),
        );
    }

    let mut lines = vec![
        "(version 1)".to_string(),
        "(deny default)".to_string(),
        "(import \"system.sb\")".to_string(),
        format!("(allow process-exec (literal {}))", escape(command)),
        "(deny signal)".to_string(),
        "(allow sysctl-read)".to_string(),
    ];
    if tree {
        // A later clause overrides an earlier one, so this narrows the blanket
        // `(deny signal)` above to the tree's own descendants: `cargo` may stop
        // a test binary it started, and may not touch anything else on the host.
        lines.push("(allow process-fork)".to_string());
        lines.push("(allow signal (target children))".to_string());
        for root in &resolved.execute_roots {
            lines.push(format!(
                "(allow process-exec (subpath {}))",
                escape(text(root)?)
            ));
        }
    }
    for tree in &trees {
        lines.push(format!("(allow file-read* (subpath {}))", escape(tree)));
    }
    lines.push(format!("(allow file-read* (literal {}))", escape(command)));
    for path in &metadata {
        lines.push(format!(
            "(allow file-read-metadata (literal {}))",
            escape(text(path)?)
        ));
    }
    for root in &resolved.write_roots {
        lines.push(format!(
            "(allow file-write* (subpath {}))",
            escape(text(root)?)
        ));
    }
    lines.push("(deny network*)".to_string());
    for path in SEATBELT_DENIED_FILES {
        lines.push(format!("(deny file-read* (literal {}))", escape(path)));
    }

    Ok(lines.join("\n"))
}

fn bubblewrap_arguments(resolved: &Resolved) -> Result<Vec<String>, ConfinementError> {
    let mut arguments: Vec<String> = [
        "--die-with-parent",
        "--new-session",
        "--unshare-all",
        "--unshare-net",
        "--clearenv",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
    ]
    .iter()
    .map(|value| value.to_string())
    .collect();

    for tree in BUBBLEWRAP_READ_TREES {
        arguments.extend([
            "--ro-bind-try".to_string(),
            tree.to_string(),
            tree.to_string(),
        ]);
    }
    arguments.extend(["--dir".to_string(), "/etc".to_string()]);
    for file in BUBBLEWRAP_LOADER_FILES {
        arguments.extend([
            "--ro-bind-try".to_string(),
            file.to_string(),
            file.to_string(),
        ]);
    }
    for root in &resolved.read_roots {
        let path = text(root)?.to_string();
        arguments.extend(["--ro-bind".to_string(), path.clone(), path]);
    }
    for root in &resolved.write_roots {
        let path = text(root)?.to_string();
        arguments.extend(["--bind".to_string(), path.clone(), path]);
    }
    // Bubblewrap has no exec filter, so a tree's executable set is bounded by
    // what the mount namespace contains: a toolchain outside every bind is not
    // merely forbidden, it is absent.
    for root in &resolved.execute_roots {
        let path = text(root)?.to_string();
        arguments.extend(["--ro-bind".to_string(), path.clone(), path]);
    }
    for (name, value) in &resolved.environment {
        arguments.extend(["--setenv".to_string(), name.clone(), value.clone()]);
    }
    arguments.extend([
        "--chdir".to_string(),
        text(&resolved.working_dir)?.to_string(),
        "--".to_string(),
        text(&resolved.command)?.to_string(),
    ]);
    arguments.extend(resolved.arguments.iter().cloned());

    Ok(arguments)
}

fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        if character == '"' || character == '\\' {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('"');
    escaped
}

fn text(path: &Path) -> Result<&str, ConfinementError> {
    let value = path
        .to_str()
        .ok_or_else(|| ConfinementError::NonUnicodePath(path.display().to_string()))?;
    if !path.is_absolute() {
        return Err(ConfinementError::RelativePath(value.to_string()));
    }
    if value.chars().any(char::is_control) {
        return Err(ConfinementError::ControlCharacterInPath(value.to_string()));
    }
    Ok(value)
}

fn canonical(path: &Path) -> Result<PathBuf, ConfinementError> {
    let canonical = fs::canonicalize(path).map_err(|error| ConfinementError::UnusablePath {
        path: path.display().to_string(),
        reason: error.to_string(),
    })?;
    text(&canonical)?;
    Ok(canonical)
}

fn canonical_roots(roots: &[PathBuf]) -> Result<Vec<PathBuf>, ConfinementError> {
    let mut canonical_roots: Vec<PathBuf> = Vec::with_capacity(roots.len());
    for root in roots {
        let root = canonical(root)?;
        if !canonical_roots.contains(&root) {
            canonical_roots.push(root);
        }
    }
    Ok(canonical_roots)
}

fn backend_executable(backend: Backend) -> Result<PathBuf, ConfinementError> {
    usable_backend(Path::new(backend.executable()))
}

fn usable_backend(path: &Path) -> Result<PathBuf, ConfinementError> {
    let unusable = |reason: String| ConfinementError::BackendUnusable {
        path: path.display().to_string(),
        reason,
    };

    // Reporting every one of these as "not installed" sends an operator to
    // reinstall a backend that is already on disk. A confined job cannot run
    // without one, so the message is the whole of the remedy.
    let metadata = fs::metadata(path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => unusable("not installed".to_string()),
        io::ErrorKind::PermissionDenied => {
            unusable("its directory is not searchable by this user".to_string())
        }
        _ => unusable(format!("cannot be inspected: {error}")),
    })?;

    if !metadata.is_file() {
        return Err(unusable("not a regular file".to_string()));
    }

    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(unusable(format!(
            "not executable (mode {:04o})",
            metadata.permissions().mode() & 0o7777
        )));
    }

    Ok(path.to_path_buf())
}

fn executable_file(path: &Path) -> Option<PathBuf> {
    let canonical = fs::canonicalize(path).ok()?;
    let metadata = fs::metadata(&canonical).ok()?;
    (metadata.is_file() && metadata.permissions().mode() & 0o111 != 0).then_some(canonical)
}

fn resolve_command(
    command: &str,
    environment: &BTreeMap<String, String>,
) -> Result<PathBuf, ConfinementError> {
    if command.contains('/') {
        return executable_file(Path::new(command))
            .ok_or_else(|| ConfinementError::CommandNotFound(command.to_string()))
            .and_then(|path| {
                text(&path)?;
                Ok(path)
            });
    }

    let search = environment
        .get(PATH)
        .cloned()
        .or_else(|| std::env::var(PATH).ok())
        .unwrap_or_else(|| SYSTEM_PATH.to_string());

    search
        .split(':')
        .filter(|directory| !directory.is_empty())
        .find_map(|directory| executable_file(&Path::new(directory).join(command)))
        .ok_or_else(|| ConfinementError::CommandNotFound(command.to_string()))
}

fn complete_environment(
    requested: &BTreeMap<String, String>,
    command: &Path,
    writable: &Path,
) -> BTreeMap<String, String> {
    let mut environment = requested.clone();
    let writable = writable.display().to_string();
    for name in [HOME, TMPDIR, TMP, TEMP] {
        environment
            .entry(name.to_string())
            .or_insert_with(|| writable.clone());
    }
    environment
        .entry(PATH.to_string())
        .or_insert_with(|| command_path(command));
    for name in [LANG, LC_ALL] {
        environment
            .entry(name.to_string())
            .or_insert_with(|| LOCALE.to_string());
    }
    environment
}

fn command_path(command: &Path) -> String {
    let mut entries: Vec<&str> = Vec::with_capacity(3);
    if let Some(directory) = command.parent().and_then(Path::to_str) {
        entries.push(directory);
    }
    for entry in SYSTEM_PATH.split(':') {
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    entries.join(":")
}

struct ProbeWorkspace {
    base: PathBuf,
    root: PathBuf,
    denied: PathBuf,
}

impl ProbeWorkspace {
    fn create() -> Result<Self, ConfinementError> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let base = std::env::temp_dir().join(format!(
            "abnegate-exec-confinement-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir(&base).map_err(probe_failure)?;
        let base = fs::canonicalize(&base).map_err(probe_failure)?;
        let root = base.join("root");
        fs::create_dir(&root).map_err(probe_failure)?;
        fs::write(root.join(PROBE_ALLOWED), PROBE_ALLOWED_CONTENT).map_err(probe_failure)?;
        let denied = base.join(PROBE_DENIED);
        fs::write(&denied, PROBE_DENIED_CONTENT).map_err(probe_failure)?;
        Ok(Self { base, root, denied })
    }
}

impl Drop for ProbeWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn probe_failure(error: std::io::Error) -> ConfinementError {
    ConfinementError::Unproven(error.to_string())
}

async fn probe_single_command() -> Result<(), ConfinementError> {
    static OUTCOME: OnceCell<Result<(), ConfinementError>> = OnceCell::const_new();
    OUTCOME.get_or_init(run_single_command_probe).await.clone()
}

async fn probe_process_tree() -> Result<(), ConfinementError> {
    static OUTCOME: OnceCell<Result<(), ConfinementError>> = OnceCell::const_new();
    OUTCOME.get_or_init(run_process_tree_probe).await.clone()
}

async fn run_single_command_probe() -> Result<(), ConfinementError> {
    let backend = HOST_BACKEND.ok_or(ConfinementError::UnsupportedPlatform)?;
    backend_executable(backend)?;
    if executable_file(Path::new(PROBE_COMMAND)).is_none() {
        return Err(ConfinementError::Unproven(format!(
            "probe command {PROBE_COMMAND} is unavailable"
        )));
    }

    let workspace = ProbeWorkspace::create()?;

    let allowed = run_confined(
        backend,
        &workspace.root,
        PROBE_COMMAND,
        vec![text(&workspace.root.join(PROBE_ALLOWED))?.to_string()],
    )
    .await?;
    if !allowed.status.success() || allowed.stdout != PROBE_ALLOWED_CONTENT.as_bytes() {
        return Err(ConfinementError::Unproven(
            "a granted read root was not readable inside the sandbox".to_string(),
        ));
    }

    for denied in [
        text(&workspace.denied)?.to_string(),
        backend.denied_system_file().to_string(),
    ] {
        let outcome = run_confined(
            backend,
            &workspace.root,
            PROBE_COMMAND,
            vec![denied.clone()],
        )
        .await?;
        if outcome.status.success() {
            return Err(ConfinementError::Unproven(format!(
                "{denied} was readable inside the sandbox"
            )));
        }
    }

    probe_network(backend, &workspace.root).await
}

async fn probe_network(backend: Backend, root: &Path) -> Result<(), ConfinementError> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(probe_failure)?;
    let port = listener.local_addr().map_err(probe_failure)?.port();

    let connected = Arc::new(AtomicBool::new(false));
    let accepted = connected.clone();
    let acceptor = tokio::spawn(async move {
        if listener.accept().await.is_ok() {
            accepted.store(true, Ordering::SeqCst);
        }
    });

    let outcome = match network_probe_command(port) {
        Some((command, arguments)) => run_confined(backend, root, &command, arguments).await,
        None => Err(ConfinementError::Unproven(
            "no command able to open a TCP connection is installed".to_string(),
        )),
    };

    tokio::time::sleep(PROBE_SETTLE).await;
    acceptor.abort();

    let outcome = outcome?;
    if connected.load(Ordering::SeqCst) {
        return Err(ConfinementError::Unproven(
            "a confined command reached a local TCP listener".to_string(),
        ));
    }
    if outcome.status.success() {
        return Err(ConfinementError::Unproven(
            "a confined command reported a successful network connection".to_string(),
        ));
    }
    Ok(())
}

/// Prove the tree claim: everything single-command mode proves, plus that a
/// forked descendant really runs, really cannot reach the network, and really
/// cannot exec outside the granted directories.
async fn run_process_tree_probe() -> Result<(), ConfinementError> {
    probe_single_command().await?;

    let backend = HOST_BACKEND.ok_or(ConfinementError::UnsupportedPlatform)?;
    if executable_file(Path::new(PROBE_SHELL)).is_none() {
        return Err(ConfinementError::Unproven(format!(
            "probe shell {PROBE_SHELL} is unavailable, so no process tree can be built"
        )));
    }

    let workspace = ProbeWorkspace::create()?;
    probe_tree_network(backend, &workspace.root).await?;
    probe_tree_execute_bound(backend, &workspace.root).await
}

/// The tree-mode counterpart of [`probe_network`]: the process that reaches for
/// the listener is a forked descendant, not the process the sandbox was applied
/// to, so a backend that confines only the direct child cannot pass this.
async fn probe_tree_network(backend: Backend, root: &Path) -> Result<(), ConfinementError> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(probe_failure)?;
    let port = listener.local_addr().map_err(probe_failure)?.port();

    let connected = Arc::new(AtomicBool::new(false));
    let accepted = connected.clone();
    let acceptor = tokio::spawn(async move {
        if listener.accept().await.is_ok() {
            accepted.store(true, Ordering::SeqCst);
        }
    });

    let outcome = match network_probe_command(port) {
        Some((command, arguments)) => {
            run_tree_network_client(backend, root, &command, &arguments).await
        }
        None => Err(ConfinementError::Unproven(
            "no command able to open a TCP connection is installed".to_string(),
        )),
    };

    tokio::time::sleep(PROBE_SETTLE).await;
    acceptor.abort();
    outcome?;

    if read_probe_marker(&root.join(PROBE_PARENT_IDENTIFIER))?
        == read_probe_marker(&root.join(PROBE_CHILD_IDENTIFIER))?
    {
        return Err(ConfinementError::Unproven(
            "the tree probe never forked, so it proves nothing about a descendant".to_string(),
        ));
    }
    if connected.load(Ordering::SeqCst) {
        return Err(ConfinementError::Unproven(
            "a forked descendant of a confined command reached a local TCP listener".to_string(),
        ));
    }
    if read_probe_marker(&root.join(PROBE_NETWORK_STATUS))? == "0" {
        return Err(ConfinementError::Unproven(
            "a forked descendant reported a successful network connection".to_string(),
        ));
    }
    Ok(())
}

/// Run the network client as a forked grandchild of the sandbox entry point.
async fn run_tree_network_client(
    backend: Backend,
    root: &Path,
    command: &str,
    arguments: &[String],
) -> Result<std::process::Output, ConfinementError> {
    let execute_roots = vec![parent_directory(PROBE_SHELL)?, parent_directory(command)?];
    write_tree_scripts(root, command, arguments)?;
    run_confined_tree(
        backend,
        root,
        PROBE_SHELL,
        vec![PROBE_PARENT_SCRIPT.to_string()],
        execute_roots,
    )
    .await
}

/// Plant a runnable executable inside a writable root and assert the tree can
/// start it only when that directory is named an execute root.
///
/// Both directions are checked. Asserting only the refusal would pass just as
/// happily if the planted file could never run at all, which is the failure
/// this whole probe exists to catch.
async fn probe_tree_execute_bound(backend: Backend, root: &Path) -> Result<(), ConfinementError> {
    if !backend.enforces_execute_roots() {
        return Ok(());
    }

    let planted = root.join(PROBE_PLANTED_COMMAND);
    fs::write(&planted, format!("#!{PROBE_SHELL}\nexit 0\n")).map_err(probe_failure)?;
    fs::set_permissions(&planted, fs::Permissions::from_mode(PROBE_PLANTED_MODE))
        .map_err(probe_failure)?;
    fs::write(
        root.join(PROBE_EXECUTE_SCRIPT),
        format!("./{PROBE_PLANTED_COMMAND}\nprintf '%s' \"$?\" > {PROBE_EXECUTE_STATUS}\n"),
    )
    .map_err(probe_failure)?;

    let granted = probe_planted_status(
        backend,
        root,
        vec![parent_directory(PROBE_SHELL)?, root.to_path_buf()],
    )
    .await?;
    if granted != "0" {
        return Err(ConfinementError::Unproven(format!(
            "the planted executable did not run even from a granted execute root ({granted}), so the refusal below would prove nothing"
        )));
    }

    let refused = probe_planted_status(backend, root, vec![parent_directory(PROBE_SHELL)?]).await?;
    if refused == "0" {
        return Err(ConfinementError::Unproven(
            "a confined tree executed a file from a writable root that was not an execute root"
                .to_string(),
        ));
    }
    Ok(())
}

async fn probe_planted_status(
    backend: Backend,
    root: &Path,
    execute_roots: Vec<PathBuf>,
) -> Result<String, ConfinementError> {
    let status = root.join(PROBE_EXECUTE_STATUS);
    let _ = fs::remove_file(&status);
    run_confined_tree(
        backend,
        root,
        PROBE_SHELL,
        vec![PROBE_EXECUTE_SCRIPT.to_string()],
        execute_roots,
    )
    .await?;
    read_probe_marker(&status)
}

/// The parent forks, the child execs the network client. The recorded process
/// identifiers are what later proves the fork happened rather than the shell
/// collapsing the chain into a single exec.
fn write_tree_scripts(
    root: &Path,
    command: &str,
    arguments: &[String],
) -> Result<(), ConfinementError> {
    let parent = format!(
        "printf '%s' \"$$\" > {PROBE_PARENT_IDENTIFIER}\n\
         {shell} {PROBE_CHILD_SCRIPT} &\n\
         wait $!\n",
        shell = shell_quote(PROBE_SHELL),
    );
    let client = std::iter::once(shell_quote(command))
        .chain(arguments.iter().map(|argument| shell_quote(argument)))
        .collect::<Vec<String>>()
        .join(" ");
    let child = format!(
        "printf '%s' \"$$\" > {PROBE_CHILD_IDENTIFIER}\n\
         {client} < /dev/null\n\
         printf '%s' \"$?\" > {PROBE_NETWORK_STATUS}\n"
    );

    fs::write(root.join(PROBE_PARENT_SCRIPT), parent).map_err(probe_failure)?;
    fs::write(root.join(PROBE_CHILD_SCRIPT), child).map_err(probe_failure)?;
    Ok(())
}

fn read_probe_marker(path: &Path) -> Result<String, ConfinementError> {
    fs::read_to_string(path)
        .map(|value| value.trim().to_string())
        .map_err(|error| {
            ConfinementError::Unproven(format!(
                "the tree probe did not record {}: {error}",
                path.display()
            ))
        })
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn parent_directory(command: &str) -> Result<PathBuf, ConfinementError> {
    let resolved = executable_file(Path::new(command))
        .ok_or_else(|| ConfinementError::CommandNotFound(command.to_string()))?;
    resolved
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| ConfinementError::CommandNotFound(command.to_string()))
}

fn network_probe_command(port: u16) -> Option<(String, Vec<String>)> {
    let command = NETWORK_PROBE_COMMANDS
        .into_iter()
        .find(|candidate| executable_file(Path::new(candidate)).is_some())?;
    let arguments = if command.ends_with("curl") {
        vec![
            "--silent".to_string(),
            "--max-time".to_string(),
            "2".to_string(),
            format!("http://127.0.0.1:{port}/"),
        ]
    } else {
        vec![
            "-w".to_string(),
            "1".to_string(),
            "127.0.0.1".to_string(),
            port.to_string(),
        ]
    };
    Some((command.to_string(), arguments))
}

async fn run_confined(
    backend: Backend,
    root: &Path,
    command: &str,
    arguments: Vec<String>,
) -> Result<std::process::Output, ConfinementError> {
    run_in_sandbox(backend, root, command, arguments, None).await
}

async fn run_confined_tree(
    backend: Backend,
    root: &Path,
    command: &str,
    arguments: Vec<String>,
    execute_roots: Vec<PathBuf>,
) -> Result<std::process::Output, ConfinementError> {
    run_in_sandbox(
        backend,
        root,
        command,
        arguments,
        Some(ProcessTreeRequest { execute_roots }),
    )
    .await
}

async fn run_in_sandbox(
    backend: Backend,
    root: &Path,
    command: &str,
    arguments: Vec<String>,
    process_tree: Option<ProcessTreeRequest>,
) -> Result<std::process::Output, ConfinementError> {
    let request = ConfinementRequest {
        read_roots: vec![root.to_path_buf()],
        write_roots: vec![root.to_path_buf()],
        process_tree,
    };
    let invocation = Confinement::new(command, arguments, root)
        .with_roots(&request)
        .invocation(Some(backend))?;

    let run = Command::new(&invocation.program)
        .args(&invocation.arguments)
        .current_dir(root)
        .env_clear()
        .envs(&invocation.environment)
        .stdin(Stdio::null())
        .output();

    match timeout(PROBE_TIMEOUT, run).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(probe_failure(error)),
        Err(_) => Err(ConfinementError::Unproven(format!(
            "the sandbox probe did not finish within {}s",
            PROBE_TIMEOUT.as_secs()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_plain_path() {
        assert_eq!(escape("/tmp/work"), "\"/tmp/work\"");
    }

    #[test]
    fn test_escape_quotes_and_backslashes() {
        assert_eq!(escape("/tmp/a\"b\\c d"), "\"/tmp/a\\\"b\\\\c d\"");
    }

    #[test]
    fn test_escape_closing_backslash_cannot_escape_the_terminator() {
        let escaped = escape("/tmp/trailing\\");
        assert_eq!(escaped, "\"/tmp/trailing\\\\\"");
        assert_eq!(escaped.matches('"').count(), 2);
    }

    #[test]
    fn test_escape_preserves_unicode() {
        assert_eq!(escape("/tmp/日本語"), "\"/tmp/日本語\"");
    }

    #[test]
    fn test_text_rejects_relative_paths() {
        assert_eq!(
            text(Path::new("relative/path")),
            Err(ConfinementError::RelativePath("relative/path".to_string()))
        );
    }

    #[test]
    fn test_text_rejects_control_characters() {
        assert!(matches!(
            text(Path::new("/tmp/a\nb")),
            Err(ConfinementError::ControlCharacterInPath(_))
        ));
    }

    #[test]
    fn an_unusable_backend_says_which_way_it_is_unusable() {
        let directory = tempfile::tempdir().expect("temporary directory");

        let absent = directory.path().join("sandbox-exec");
        assert_eq!(
            usable_backend(&absent),
            Err(ConfinementError::BackendUnusable {
                path: absent.display().to_string(),
                reason: "not installed".to_string(),
            })
        );

        let not_a_file = directory.path().join("subdirectory");
        fs::create_dir(&not_a_file).expect("directory is created");
        assert_eq!(
            usable_backend(&not_a_file),
            Err(ConfinementError::BackendUnusable {
                path: not_a_file.display().to_string(),
                reason: "not a regular file".to_string(),
            })
        );

        // The case the old message got wrong: the backend is installed, so
        // "not installed" sends the operator to reinstall what is already here.
        let unreadable = directory.path().join("not-executable");
        fs::write(&unreadable, b"#!/bin/sh\n").expect("file is written");
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644))
            .expect("permissions are set");
        assert_eq!(
            usable_backend(&unreadable),
            Err(ConfinementError::BackendUnusable {
                path: unreadable.display().to_string(),
                reason: "not executable (mode 0644)".to_string(),
            })
        );

        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o755))
            .expect("permissions are set");
        assert_eq!(usable_backend(&unreadable), Ok(unreadable));
    }

    #[test]
    fn test_backend_executables() {
        assert_eq!(Backend::Seatbelt.executable(), SEATBELT);
        assert_eq!(Backend::Bubblewrap.executable(), BUBBLEWRAP);
    }

    #[test]
    fn test_command_path_prefixes_the_command_directory() {
        assert_eq!(
            command_path(Path::new("/opt/tools/bin/run")),
            "/opt/tools/bin:/usr/bin:/bin"
        );
    }

    #[test]
    fn test_command_path_does_not_repeat_system_entries() {
        assert_eq!(command_path(Path::new("/bin/cat")), "/bin:/usr/bin");
    }

    #[test]
    fn test_complete_environment_fills_defaults() {
        let environment = complete_environment(
            &BTreeMap::new(),
            Path::new("/bin/cat"),
            Path::new("/tmp/writable"),
        );
        assert_eq!(environment.get(HOME).unwrap(), "/tmp/writable");
        assert_eq!(environment.get(TMPDIR).unwrap(), "/tmp/writable");
        assert_eq!(environment.get(LC_ALL).unwrap(), LOCALE);
    }

    #[test]
    fn test_complete_environment_keeps_requested_values() {
        let requested = BTreeMap::from([(HOME.to_string(), "/tmp/mine".to_string())]);
        let environment = complete_environment(
            &requested,
            Path::new("/bin/cat"),
            Path::new("/tmp/writable"),
        );
        assert_eq!(environment.get(HOME).unwrap(), "/tmp/mine");
    }

    #[test]
    fn test_resolve_command_finds_absolute_path() {
        let resolved = resolve_command("/bin/cat", &BTreeMap::new()).unwrap();
        assert!(resolved.ends_with("cat"));
    }

    #[test]
    fn test_resolve_command_rejects_missing_command() {
        assert_eq!(
            resolve_command("/bin/definitely-not-a-command", &BTreeMap::new()),
            Err(ConfinementError::CommandNotFound(
                "/bin/definitely-not-a-command".to_string()
            ))
        );
    }

    #[test]
    fn test_resolve_command_searches_the_requested_path() {
        let environment = BTreeMap::from([(PATH.to_string(), "/bin".to_string())]);
        assert!(resolve_command("cat", &environment).is_ok());
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
