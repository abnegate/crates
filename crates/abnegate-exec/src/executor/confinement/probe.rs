use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::OnceCell;
use tokio::time::timeout;

use crate::protocol::ConfinementRequest;
use crate::protocol::ProcessTreeRequest;

use super::Confinement;
use super::backend::Backend;
use super::backend::HOST_BACKEND;
use super::backend::backend_executable;
use super::error::ConfinementError;
use super::mode::ConfinementMode;
use super::path::executable_file;
use super::path::text;
use super::workspace::ProbeWorkspace;

const PROBE_COMMAND: &str = "/bin/cat";
pub(super) const PROBE_ALLOWED: &str = "allowed";
pub(super) const PROBE_DENIED: &str = "denied";
pub(super) const PROBE_ALLOWED_CONTENT: &str = "allowed\n";
pub(super) const PROBE_DENIED_CONTENT: &str = "secret\n";
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

pub(super) fn probe_failure(error: std::io::Error) -> ConfinementError {
    ConfinementError::Unproven(error.to_string())
}

pub(super) async fn probe_single_command() -> Result<(), ConfinementError> {
    static OUTCOME: OnceCell<Result<(), ConfinementError>> = OnceCell::const_new();
    OUTCOME.get_or_init(run_single_command_probe).await.clone()
}

pub(super) async fn probe_process_tree() -> Result<(), ConfinementError> {
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
    let backend = HOST_BACKEND.ok_or(ConfinementError::UnsupportedPlatform)?;
    backend.require(ConfinementMode::ProcessTree)?;
    probe_single_command().await?;

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

    let child = invocation
        .spawn(|command| {
            command
                .current_dir(root)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
        })
        .map_err(probe_failure)?;

    match timeout(PROBE_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(probe_failure(error)),
        Err(_) => Err(ConfinementError::Unproven(format!(
            "the sandbox probe did not finish within {}s",
            PROBE_TIMEOUT.as_secs()
        ))),
    }
}
