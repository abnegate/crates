//! Integration tests for OS-level confinement.
//!
//! These tests exercise:
//! - Seatbelt profile generation, including path escaping
//! - Bubblewrap argument generation
//! - The unsupported-platform path
//! - Fail-closed spawning when confinement cannot be established
//! - Real confined execution on hosts that can prove their sandbox

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use abnegate_exec::error::ExecutorError;
use abnegate_exec::executor::Backend;
use abnegate_exec::executor::CommandExecutor;
use abnegate_exec::executor::Confinement;
use abnegate_exec::executor::ConfinementError;
use abnegate_exec::executor::ConfinementMode;
use abnegate_exec::executor::HOST_BACKEND;
use abnegate_exec::protocol::Capability;
use abnegate_exec::protocol::ConfinementRequest;
use abnegate_exec::protocol::ErrorCode;
use abnegate_exec::protocol::InboundMessage;
use abnegate_exec::protocol::OutboundMessage;
use abnegate_exec::protocol::ProcessTreeRequest;
use base64::prelude::*;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

const SECRET: &str = "secret\n";
const GRANTED: &str = "granted\n";

struct Workspace {
    _base: TempDir,
    root: PathBuf,
    granted: PathBuf,
    denied: PathBuf,
}

/// A readable/writable root holding `granted`, next to a `denied` file that no
/// confined command is allowed to see.
fn workspace() -> Workspace {
    let base = TempDir::new().unwrap();
    let root = fs::canonicalize(base.path()).unwrap().join("root");
    fs::create_dir(&root).unwrap();
    let granted = root.join("granted");
    fs::write(&granted, GRANTED).unwrap();
    let denied = fs::canonicalize(base.path()).unwrap().join("denied");
    fs::write(&denied, SECRET).unwrap();

    Workspace {
        _base: base,
        root,
        granted,
        denied,
    }
}

fn request(root: &Path) -> ConfinementRequest {
    ConfinementRequest {
        read_roots: vec![root.to_path_buf()],
        write_roots: vec![root.to_path_buf()],
        process_tree: None,
    }
}

fn confinement(root: &Path, arguments: Vec<String>) -> Confinement {
    Confinement::new("/bin/cat", arguments, root).with_roots(&request(root))
}

fn text(path: &Path) -> String {
    path.to_str().unwrap().to_string()
}

fn confined_run(job_id: &str, root: &Path, target: &Path) -> InboundMessage {
    InboundMessage::RunStart {
        job_id: job_id.to_string(),
        workspace: root.to_path_buf(),
        command: "/bin/cat".to_string(),
        args: vec![text(target)],
        env: HashMap::new(),
        timeout_ms: Some(15000),
        max_output_bytes: None,
        working_dir: None,
        confinement: Some(Box::new(request(root))),
    }
}

async fn collect_messages(
    rx: &mut mpsc::Receiver<OutboundMessage>,
    timeout: Duration,
) -> Vec<OutboundMessage> {
    let mut messages = Vec::new();
    let deadline = tokio::time::Instant::now() + timeout;

    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(message)) => {
                let finished = matches!(
                    message,
                    OutboundMessage::RunExit { .. } | OutboundMessage::RunError { .. }
                );
                messages.push(message);
                if finished {
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }

    messages
}

fn stdout(messages: &[OutboundMessage]) -> String {
    messages
        .iter()
        .filter_map(|message| match message {
            OutboundMessage::RunStdout { data, .. } => Some(data),
            _ => None,
        })
        .map(|data| String::from_utf8(BASE64_STANDARD.decode(data).unwrap()).unwrap())
        .collect()
}

fn exit_code(messages: &[OutboundMessage]) -> Option<i32> {
    messages.iter().find_map(|message| match message {
        OutboundMessage::RunExit { exit_code, .. } => *exit_code,
        _ => None,
    })
}

fn seatbelt_profile(confinement: &Confinement) -> String {
    let invocation = confinement.invocation(Some(Backend::Seatbelt)).unwrap();
    assert_eq!(invocation.program, PathBuf::from("/usr/bin/sandbox-exec"));
    assert_eq!(invocation.arguments[0], "-p");
    assert_eq!(invocation.arguments[2], "--");
    invocation.arguments[1].clone()
}

#[test]
fn test_seatbelt_profile_starts_from_a_full_deny() {
    let workspace = workspace();
    let profile = seatbelt_profile(&confinement(&workspace.root, vec![]));
    let lines: Vec<&str> = profile.lines().collect();

    assert_eq!(lines[0], "(version 1)");
    assert_eq!(lines[1], "(deny default)");
    assert_eq!(lines[2], "(import \"system.sb\")");
}

#[test]
fn test_seatbelt_profile_denies_network_signals_and_host_files() {
    let workspace = workspace();
    let profile = seatbelt_profile(&confinement(&workspace.root, vec![]));

    assert!(profile.contains("(deny network*)"), "{profile}");
    assert!(profile.contains("(deny signal)"), "{profile}");
    assert!(profile.contains("(allow sysctl-read)"), "{profile}");
    assert!(
        profile.contains(r#"(deny file-read* (literal "/private/etc/hosts"))"#),
        "{profile}"
    );
    assert!(
        profile.contains(r#"(deny file-read* (literal "/private/etc/passwd"))"#),
        "{profile}"
    );
}

#[test]
fn test_seatbelt_profile_grants_the_requested_roots() {
    let workspace = workspace();
    let profile = seatbelt_profile(&confinement(&workspace.root, vec![]));
    let root = text(&workspace.root);

    assert!(
        profile.contains(&format!("(allow file-read* (subpath \"{root}\"))")),
        "{profile}"
    );
    assert!(
        profile.contains(&format!("(allow file-write* (subpath \"{root}\"))")),
        "{profile}"
    );
    for tree in ["/System", "/dev", "/usr/lib", "/usr/share"] {
        assert!(
            profile.contains(&format!("(allow file-read* (subpath \"{tree}\"))")),
            "{profile}"
        );
    }
}

#[test]
fn test_seatbelt_profile_pins_the_executable_and_its_ancestors() {
    let workspace = workspace();
    let profile = seatbelt_profile(&confinement(&workspace.root, vec![]));
    let command = fs::canonicalize("/bin/cat").unwrap();
    let command = text(&command);

    assert!(
        profile.contains(&format!("(allow process-exec (literal \"{command}\"))")),
        "{profile}"
    );
    assert!(
        profile.contains(&format!("(allow file-read* (literal \"{command}\"))")),
        "{profile}"
    );
    for ancestor in Path::new(&command)
        .ancestors()
        .filter(|ancestor| ancestor.parent().is_some())
    {
        assert!(
            profile.contains(&format!(
                "(allow file-read-metadata (literal \"{}\"))",
                text(ancestor)
            )),
            "missing metadata for {}\n{profile}",
            ancestor.display()
        );
    }
}

#[test]
fn test_seatbelt_profile_escapes_quotes_backslashes_and_spaces() {
    let base = TempDir::new().unwrap();
    let root = fs::canonicalize(base.path())
        .unwrap()
        .join(r#"odd " name \ with spaces"#);
    fs::create_dir(&root).unwrap();

    let profile = seatbelt_profile(&confinement(&root, vec![]));
    let escaped = text(&root).replace('\\', r"\\").replace('"', "\\\"");

    assert!(
        profile.contains(&format!("(allow file-read* (subpath \"{escaped}\"))")),
        "{profile}"
    );
    assert!(
        profile.contains(&format!("(allow file-write* (subpath \"{escaped}\"))")),
        "{profile}"
    );
    assert!(
        !profile.contains(&format!("subpath \"{}\"", text(&root))),
        "the raw path escaped its string literal\n{profile}"
    );

    for line in profile.lines() {
        let quotes = line
            .char_indices()
            .filter(|(index, character)| *character == '"' && !line[..*index].ends_with('\\'))
            .count();
        assert!(quotes % 2 == 0, "unbalanced string literal in: {line}");
    }
}

#[test]
fn test_seatbelt_profile_rejects_a_newline_root_instead_of_injecting_a_clause() {
    let base = TempDir::new().unwrap();
    let base = fs::canonicalize(base.path()).unwrap();
    let root = base.join("in\n(allow default)\njected");
    fs::create_dir(&root).unwrap();

    let confinement = Confinement::new("/bin/cat", vec![], &base).with_roots(&ConfinementRequest {
        read_roots: vec![root],
        write_roots: vec![],
        process_tree: None,
    });

    assert!(matches!(
        confinement.invocation(Some(Backend::Seatbelt)),
        Err(ConfinementError::ControlCharacterInPath(_))
    ));
}

#[test]
fn test_confinement_rejects_a_root_that_does_not_exist() {
    let workspace = workspace();
    let confinement =
        Confinement::new("/bin/cat", vec![], &workspace.root).with_roots(&ConfinementRequest {
            read_roots: vec![workspace.root.join("missing")],
            write_roots: vec![],
            process_tree: None,
        });

    assert!(matches!(
        confinement.invocation(Some(Backend::Seatbelt)),
        Err(ConfinementError::UnusablePath { .. })
    ));
}

#[test]
fn test_confinement_rejects_a_relative_root() {
    let workspace = workspace();
    let confinement =
        Confinement::new("/bin/cat", vec![], &workspace.root).with_roots(&ConfinementRequest {
            read_roots: vec![PathBuf::from("relative/root")],
            write_roots: vec![],
            process_tree: None,
        });

    assert!(matches!(
        confinement.invocation(Some(Backend::Seatbelt)),
        Err(ConfinementError::UnusablePath { .. })
    ));
}

fn bubblewrap_arguments(confinement: &Confinement) -> Vec<String> {
    let invocation = confinement.invocation(Some(Backend::Bubblewrap)).unwrap();
    assert_eq!(invocation.program, PathBuf::from("/usr/bin/bwrap"));
    assert!(
        invocation.environment.is_empty(),
        "bubblewrap carries the environment through --setenv"
    );
    invocation.arguments
}

fn window(arguments: &[String], values: &[&str]) -> bool {
    arguments
        .windows(values.len())
        .any(|slice| slice.iter().zip(values).all(|(left, right)| left == right))
}

#[test]
fn test_bubblewrap_arguments_unshare_everything() {
    let workspace = workspace();
    let arguments = bubblewrap_arguments(&confinement(&workspace.root, vec![]));

    assert_eq!(
        arguments[..11],
        [
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
    );
}

#[test]
fn test_bubblewrap_arguments_bind_system_trees_read_only() {
    let workspace = workspace();
    let arguments = bubblewrap_arguments(&confinement(&workspace.root, vec![]));

    for tree in ["/usr", "/bin", "/lib", "/lib64"] {
        assert!(window(&arguments, &["--ro-bind-try", tree, tree]), "{tree}");
    }
    assert!(window(&arguments, &["--dir", "/etc"]));
    for file in ["/etc/ld.so.cache", "/etc/ld.so.conf", "/etc/localtime"] {
        assert!(window(&arguments, &["--ro-bind-try", file, file]), "{file}");
    }
}

#[test]
fn test_bubblewrap_arguments_bind_the_requested_roots() {
    let workspace = workspace();
    let arguments = bubblewrap_arguments(&confinement(&workspace.root, vec![]));
    let root = text(&workspace.root);

    assert!(window(&arguments, &["--ro-bind", &root, &root]));
    assert!(window(&arguments, &["--bind", &root, &root]));
}

#[test]
fn test_bubblewrap_arguments_set_the_environment_and_working_directory() {
    let workspace = workspace();
    let arguments = bubblewrap_arguments(&confinement(&workspace.root, vec![]));
    let root = text(&workspace.root);

    for name in ["HOME", "TMPDIR", "TMP", "TEMP"] {
        assert!(window(&arguments, &["--setenv", name, &root]), "{name}");
    }
    assert!(window(&arguments, &["--setenv", "LANG", "C.UTF-8"]));
    assert!(window(&arguments, &["--setenv", "LC_ALL", "C.UTF-8"]));
    assert!(window(&arguments, &["--chdir", &root]));
}

#[test]
fn test_bubblewrap_arguments_end_with_the_command_and_its_arguments() {
    let workspace = workspace();
    let arguments = bubblewrap_arguments(&confinement(
        &workspace.root,
        vec![text(&workspace.granted)],
    ));
    let command = text(&fs::canonicalize("/bin/cat").unwrap());

    let separator = arguments.iter().position(|value| value == "--").unwrap();
    assert_eq!(arguments[separator + 1], command);
    assert_eq!(arguments[separator + 2], text(&workspace.granted));
    assert_eq!(arguments.len(), separator + 3);
}

#[test]
fn test_bubblewrap_arguments_keep_a_caller_supplied_environment() {
    let workspace = workspace();
    let confinement = confinement(&workspace.root, vec![])
        .with_environment(HashMap::from([("HOME".to_string(), "/tmp".to_string())]));
    let arguments = bubblewrap_arguments(&confinement);

    assert!(window(&arguments, &["--setenv", "HOME", "/tmp"]));
}

#[test]
fn test_unsupported_platform_yields_an_error_not_an_unconfined_command() {
    let workspace = workspace();
    let confinement = confinement(&workspace.root, vec![]);

    assert_eq!(
        confinement.invocation(None),
        Err(ConfinementError::UnsupportedPlatform)
    );
}

#[test]
fn test_capability_is_advertised_only_when_a_backend_exists() {
    let advertised = Capability::supported().contains(&"confinement".to_string());

    assert_eq!(advertised, Confinement::is_available());
    assert!(Capability::all().contains(&"confinement".to_string()));
}

#[test]
fn test_run_start_carries_a_confinement_request() {
    let json = r#"{
        "type": "RunStart",
        "job_id": "confined",
        "workspace": "/tmp",
        "command": "ls",
        "confinement": {
            "read_roots": ["/tmp/source"],
            "write_roots": ["/tmp/output"]
        }
    }"#;

    let message: InboundMessage = serde_json::from_str(json).unwrap();
    match message {
        InboundMessage::RunStart { confinement, .. } => {
            let confinement = confinement.unwrap();
            assert_eq!(confinement.read_roots, vec![PathBuf::from("/tmp/source")]);
            assert_eq!(confinement.write_roots, vec![PathBuf::from("/tmp/output")]);
        }
        _ => panic!("Wrong message type"),
    }
}

#[test]
fn test_run_start_without_confinement_stays_unconfined() {
    let json = r#"{"type": "RunStart", "job_id": "plain", "workspace": "/tmp", "command": "ls"}"#;
    let message: InboundMessage = serde_json::from_str(json).unwrap();

    match message {
        InboundMessage::RunStart { confinement, .. } => assert!(confinement.is_none()),
        _ => panic!("Wrong message type"),
    }
}

#[test]
fn test_confinement_unavailable_has_an_error_code() {
    let message = OutboundMessage::error(
        "job-1",
        ErrorCode::ConfinementUnavailable,
        "no backend on this host",
    );
    let json = serde_json::to_string(&message).unwrap();

    assert!(
        json.contains(r#""error_code":"confinement_unavailable""#),
        "{json}"
    );
}

#[tokio::test]
async fn test_spawn_fails_closed_when_confinement_cannot_be_established() {
    let workspace = workspace();
    let (tx, mut rx) = mpsc::channel(100);

    let request = InboundMessage::RunStart {
        job_id: "unprovable".to_string(),
        workspace: workspace.root.clone(),
        command: "/bin/cat".to_string(),
        args: vec![text(&workspace.granted)],
        env: HashMap::new(),
        timeout_ms: Some(15000),
        max_output_bytes: None,
        working_dir: None,
        confinement: Some(Box::new(ConfinementRequest {
            read_roots: vec![workspace.root.join("does-not-exist")],
            write_roots: vec![],
            process_tree: None,
        })),
    };

    let result = CommandExecutor::new().spawn(&request, tx).await;

    match result {
        Err(error @ ExecutorError::ConfinementUnavailable(_)) => {
            assert_eq!(error.to_error_code(), ErrorCode::ConfinementUnavailable);
        }
        Err(error) => panic!("Wrong error type: {error:?}"),
        Ok(_) => panic!("An unprovable confinement must not spawn the command"),
    }

    assert!(
        collect_messages(&mut rx, Duration::from_millis(200))
            .await
            .is_empty(),
        "A refused spawn must not report a started process"
    );
}

#[tokio::test]
async fn test_probe_result_is_cached() {
    let first = Confinement::probe(ConfinementMode::SingleCommand).await;
    let second = Confinement::probe(ConfinementMode::SingleCommand).await;

    assert_eq!(first, second);
}

async fn run_confined(request: &InboundMessage) -> Vec<OutboundMessage> {
    let (tx, mut rx) = mpsc::channel(1000);
    CommandExecutor::new().spawn(request, tx).await.unwrap();
    collect_messages(&mut rx, Duration::from_secs(20)).await
}

#[tokio::test]
async fn test_confined_command_reads_a_granted_root() {
    if Confinement::probe(ConfinementMode::SingleCommand)
        .await
        .is_err()
    {
        return;
    }

    let workspace = workspace();
    let messages = run_confined(&confined_run(
        "granted",
        &workspace.root,
        &workspace.granted,
    ))
    .await;

    assert_eq!(exit_code(&messages), Some(0), "{messages:?}");
    assert_eq!(stdout(&messages), GRANTED);
}

#[tokio::test]
async fn test_confined_command_cannot_read_outside_its_roots() {
    if Confinement::probe(ConfinementMode::SingleCommand)
        .await
        .is_err()
    {
        return;
    }

    let workspace = workspace();
    let messages = run_confined(&confined_run("denied", &workspace.root, &workspace.denied)).await;

    assert_ne!(exit_code(&messages), Some(0), "{messages:?}");
    assert!(
        !stdout(&messages).contains(SECRET.trim()),
        "the sandbox leaked a file outside its read roots"
    );
}

/// Point `client` at a fresh local listener and report whether the connection
/// arrived, plus the exit code the client reported.
async fn attempt_connection(root: &Path, client: &str, confined: bool) -> (bool, Option<i32>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let connected = Arc::new(AtomicBool::new(false));
    let accepted = connected.clone();
    let acceptor = tokio::spawn(async move {
        if listener.accept().await.is_ok() {
            accepted.store(true, Ordering::SeqCst);
        }
    });

    let args = if client.ends_with("curl") {
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

    let messages = run_confined(&InboundMessage::RunStart {
        job_id: format!("network-{confined}"),
        workspace: root.to_path_buf(),
        command: client.to_string(),
        args,
        env: HashMap::new(),
        timeout_ms: Some(15000),
        max_output_bytes: None,
        working_dir: None,
        confinement: confined.then(|| Box::new(request(root))),
    })
    .await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    acceptor.abort();

    (connected.load(Ordering::SeqCst), exit_code(&messages))
}

#[tokio::test]
async fn test_confinement_blocks_a_connection_that_otherwise_succeeds() {
    if Confinement::probe(ConfinementMode::SingleCommand)
        .await
        .is_err()
    {
        return;
    }
    let Some(client) = ["/usr/bin/nc", "/bin/nc", "/usr/bin/curl"]
        .into_iter()
        .find(|candidate| Path::new(candidate).is_file())
    else {
        return;
    };

    let workspace = workspace();

    let (unconfined, _) = attempt_connection(&workspace.root, client, false).await;
    assert!(
        unconfined,
        "the unconfined control never reached the listener, so this test proves nothing"
    );

    let (confined, code) = attempt_connection(&workspace.root, client, true).await;
    assert!(!confined, "a confined command reached a local TCP listener");
    assert_ne!(code, Some(0));
}

const SHELL: &str = "/bin/sh";
const SHELL_DIRECTORY: &str = "/bin";

/// The execute root as the confinement will name it.
///
/// `/bin` is a symlink to `/usr/bin` on a usrmerge Linux, and roots are
/// canonicalised so containment is a real prefix check rather than a string
/// one. An assertion that expects the literal passes on macOS and fails on CI.
fn resolved_shell_directory() -> String {
    std::fs::canonicalize(SHELL_DIRECTORY)
        .expect("the shell directory resolves")
        .display()
        .to_string()
}
const PARENT_SCRIPT: &str = "parent.sh";
const CHILD_SCRIPT: &str = "child.sh";
const PARENT_IDENTIFIER: &str = "parent.pid";
const CHILD_IDENTIFIER: &str = "child.pid";

fn tree_request(root: &Path, execute_roots: Vec<PathBuf>) -> ConfinementRequest {
    ConfinementRequest {
        read_roots: vec![root.to_path_buf()],
        write_roots: vec![root.to_path_buf()],
        process_tree: Some(ProcessTreeRequest { execute_roots }),
    }
}

fn tree_confinement(root: &Path, execute_roots: Vec<PathBuf>) -> Confinement {
    Confinement::new("/bin/cat", vec![], root).with_roots(&tree_request(root, execute_roots))
}

/// A parent that forks a child, so the two recorded process identifiers differ
/// only if a real fork happened.
fn write_forking_scripts(root: &Path, child_body: &str) {
    fs::write(
        root.join(PARENT_SCRIPT),
        format!(
            "printf '%s' \"$$\" > {PARENT_IDENTIFIER}\n{SHELL} {CHILD_SCRIPT} &\nwait $!\nexit $?\n"
        ),
    )
    .unwrap();
    fs::write(
        root.join(CHILD_SCRIPT),
        format!("printf '%s' \"$$\" > {CHILD_IDENTIFIER}\n{child_body}"),
    )
    .unwrap();
}

fn marker(root: &Path, name: &str) -> Option<String> {
    fs::read_to_string(root.join(name))
        .ok()
        .map(|value| value.trim().to_string())
}

fn confined_tree_run(
    job_id: &str,
    root: &Path,
    script: &str,
    execute_roots: Vec<PathBuf>,
) -> InboundMessage {
    InboundMessage::RunStart {
        job_id: job_id.to_string(),
        workspace: root.to_path_buf(),
        command: SHELL.to_string(),
        args: vec![script.to_string()],
        env: HashMap::new(),
        timeout_ms: Some(20000),
        max_output_bytes: None,
        working_dir: None,
        confinement: Some(Box::new(tree_request(root, execute_roots))),
    }
}

#[test]
fn test_seatbelt_tree_profile_admits_a_bounded_tree() {
    let workspace = workspace();
    let profile = seatbelt_profile(&tree_confinement(
        &workspace.root,
        vec![PathBuf::from(SHELL_DIRECTORY)],
    ));

    assert!(profile.contains("(allow process-fork)"), "{profile}");
    let directory = resolved_shell_directory();
    assert!(
        profile.contains(&format!("(allow process-exec (subpath \"{directory}\"))")),
        "{profile}"
    );
    assert!(
        profile.contains(&format!("(allow file-read* (subpath \"{directory}\"))")),
        "an execute root the tree cannot read is an execute root it cannot use\n{profile}"
    );
    assert!(profile.contains("(deny network*)"), "{profile}");
}

#[test]
fn test_seatbelt_tree_profile_narrows_signals_to_the_trees_own_children() {
    let workspace = workspace();
    let profile = seatbelt_profile(&tree_confinement(
        &workspace.root,
        vec![PathBuf::from(SHELL_DIRECTORY)],
    ));

    let denied = profile.find("(deny signal)").expect("{profile}");
    let allowed = profile
        .find("(allow signal (target children))")
        .expect("{profile}");
    assert!(
        denied < allowed,
        "a later clause wins in seatbelt, so the narrowing must follow the blanket deny\n{profile}"
    );
}

#[test]
fn test_seatbelt_single_command_profile_still_admits_no_tree() {
    let workspace = workspace();
    let profile = seatbelt_profile(&confinement(&workspace.root, vec![]));

    assert!(
        !profile.contains("(allow process-fork)"),
        "single-command mode must not gain the right to fork\n{profile}"
    );
    assert!(
        !profile.contains("(allow process-exec (subpath"),
        "single-command mode execs one literal command and nothing else\n{profile}"
    );
    assert!(
        !profile.contains("(allow signal"),
        "single-command mode signals nothing\n{profile}"
    );
    assert!(profile.contains("(deny signal)"), "{profile}");
}

#[test]
fn test_seatbelt_tree_profile_does_not_make_a_write_root_executable() {
    let workspace = workspace();
    let profile = seatbelt_profile(&tree_confinement(
        &workspace.root,
        vec![PathBuf::from(SHELL_DIRECTORY)],
    ));
    let root = text(&workspace.root);

    assert!(
        profile.contains(&format!("(allow file-write* (subpath \"{root}\"))")),
        "{profile}"
    );
    assert!(
        !profile.contains(&format!("(allow process-exec (subpath \"{root}\"))")),
        "a writable root must not become executable by implication\n{profile}"
    );
}

#[test]
fn test_bubblewrap_tree_arguments_bind_the_execute_roots() {
    let workspace = workspace();
    let arguments = bubblewrap_arguments(&tree_confinement(
        &workspace.root,
        vec![PathBuf::from(SHELL_DIRECTORY)],
    ));

    let directory = resolved_shell_directory();
    assert!(
        window(&arguments, &["--ro-bind", &directory, &directory]),
        "{arguments:?}"
    );
    assert!(
        arguments.contains(&"--unshare-net".to_string()),
        "the network stays unshared for the whole namespace, tree or not\n{arguments:?}"
    );
    assert!(
        arguments.contains(&"--unshare-all".to_string()),
        "{arguments:?}"
    );
}

#[test]
fn test_bubblewrap_tree_arguments_bind_an_execute_root_read_only() {
    let base = TempDir::new().unwrap();
    let toolchain = fs::canonicalize(base.path()).unwrap().join("toolchain");
    fs::create_dir(&toolchain).unwrap();
    let workspace = workspace();

    let arguments =
        bubblewrap_arguments(&tree_confinement(&workspace.root, vec![toolchain.clone()]));
    let toolchain = text(&toolchain);

    assert!(
        window(&arguments, &["--ro-bind", &toolchain, &toolchain]),
        "{arguments:?}"
    );
    assert!(
        !window(&arguments, &["--bind", &toolchain, &toolchain]),
        "a toolchain a tree may execute is not a toolchain it may rewrite\n{arguments:?}"
    );
}

#[test]
fn test_a_process_tree_without_execute_roots_is_refused() {
    let workspace = workspace();
    let confinement = tree_confinement(&workspace.root, vec![]);

    assert_eq!(
        confinement.invocation(Some(Backend::Seatbelt)),
        Err(ConfinementError::ProcessTreeWithoutExecuteRoots)
    );
    assert_eq!(
        confinement.invocation(Some(Backend::Bubblewrap)),
        Err(ConfinementError::ProcessTreeWithoutExecuteRoots)
    );
}

#[test]
fn test_an_execute_root_of_the_filesystem_root_is_refused() {
    let workspace = workspace();
    let confinement = tree_confinement(&workspace.root, vec![PathBuf::from("/")]);

    assert_eq!(
        confinement.invocation(Some(Backend::Seatbelt)),
        Err(ConfinementError::UnboundedExecuteRoot("/".to_string())),
        "an execute root of / is 'allow every exec', which is not a bound"
    );
}

#[test]
fn test_an_execute_root_that_does_not_exist_is_refused() {
    let workspace = workspace();
    let confinement = tree_confinement(&workspace.root, vec![workspace.root.join("no-toolchain")]);

    assert!(matches!(
        confinement.invocation(Some(Backend::Seatbelt)),
        Err(ConfinementError::UnusablePath { .. })
    ));
}

#[test]
fn test_unsupported_platform_yields_an_error_for_a_tree_too() {
    let workspace = workspace();
    let confinement = tree_confinement(&workspace.root, vec![PathBuf::from(SHELL_DIRECTORY)]);

    assert_eq!(
        confinement.invocation(None),
        Err(ConfinementError::UnsupportedPlatform),
        "a host with no backend refuses the tree rather than running it unconfined"
    );
}

#[test]
fn test_run_start_carries_a_process_tree_request() {
    let json = r#"{
        "type": "RunStart",
        "job_id": "tree",
        "workspace": "/tmp",
        "command": "cargo",
        "confinement": {
            "read_roots": ["/tmp/source"],
            "write_roots": ["/tmp/source/target"],
            "process_tree": { "execute_roots": ["/usr/bin", "/tmp/source/target/debug/deps"] }
        }
    }"#;

    let message: InboundMessage = serde_json::from_str(json).unwrap();
    match message {
        InboundMessage::RunStart { confinement, .. } => {
            let confinement = confinement.unwrap();
            let tree = confinement.process_tree.as_ref().unwrap();
            assert_eq!(
                tree.execute_roots,
                vec![
                    PathBuf::from("/usr/bin"),
                    PathBuf::from("/tmp/source/target/debug/deps")
                ]
            );
        }
        _ => panic!("Wrong message type"),
    }
}

#[test]
fn test_a_confinement_request_without_a_process_tree_stays_single_command() {
    let json = r#"{"read_roots": ["/tmp"], "write_roots": []}"#;
    let request: ConfinementRequest = serde_json::from_str(json).unwrap();

    assert!(request.process_tree.is_none());
    assert_eq!(request.mode(), ConfinementMode::SingleCommand);
}

#[test]
fn test_a_single_command_request_serialises_without_the_tree_field() {
    let request = ConfinementRequest {
        read_roots: vec![PathBuf::from("/tmp")],
        write_roots: vec![],
        process_tree: None,
    };
    let json = serde_json::to_string(&request).unwrap();

    assert!(
        !json.contains("process_tree"),
        "the field is additive and absent by default, so a client that predates it is unaffected: {json}"
    );
}

#[test]
fn test_the_process_tree_capability_is_advertised_only_when_a_backend_exists() {
    let advertised = Capability::supported().contains(&"confinement_process_tree".to_string());

    assert_eq!(advertised, Confinement::is_available());
    assert!(Capability::all().contains(&"confinement_process_tree".to_string()));
}

#[tokio::test]
async fn test_the_tree_probe_verdict_is_cached() {
    let first = Confinement::probe(ConfinementMode::ProcessTree).await;
    let second = Confinement::probe(ConfinementMode::ProcessTree).await;

    assert_eq!(first, second);
}

/// The self-test that carries the whole claim: on a host with a backend the
/// tree probe must pass, and on a host without one it must fail rather than
/// quietly downgrade.
#[tokio::test]
async fn test_the_tree_probe_proves_or_refuses_the_tree_claim() {
    let verdict = Confinement::probe(ConfinementMode::ProcessTree).await;

    if Confinement::is_available() {
        assert_eq!(
            verdict,
            Ok(()),
            "this host has a backend, so the tree claim must be provable"
        );
    } else {
        assert!(
            verdict.is_err(),
            "a host without a backend must refuse the tree, not assume it"
        );
    }
}

#[tokio::test]
async fn test_a_confined_tree_really_forks() {
    if Confinement::probe(ConfinementMode::ProcessTree)
        .await
        .is_err()
    {
        return;
    }

    let workspace = workspace();
    write_forking_scripts(&workspace.root, "printf 'ran' > child.marker\n");

    let messages = run_confined(&confined_tree_run(
        "tree-forks",
        &workspace.root,
        PARENT_SCRIPT,
        vec![PathBuf::from(SHELL_DIRECTORY)],
    ))
    .await;

    assert_eq!(exit_code(&messages), Some(0), "{messages:?}");
    assert_eq!(
        marker(&workspace.root, "child.marker").as_deref(),
        Some("ran")
    );
    let parent = marker(&workspace.root, PARENT_IDENTIFIER).expect("parent identifier");
    let child = marker(&workspace.root, CHILD_IDENTIFIER).expect("child identifier");
    assert_ne!(
        parent, child,
        "the child ran in the parent's process, so this proves nothing about a tree"
    );
}

/// Single-command mode asks for one process, and what the host can promise
/// about that depends on the backend. Seatbelt filters `process-fork`, so the
/// second process never exists. Bubblewrap has no such primitive, so the fork
/// succeeds -- and what has to hold there instead is that the child is inside
/// the same sandbox. Asserting only "no second process" would pass vacuously on
/// the backend that cannot deliver it, which is the more dangerous of the two.
#[tokio::test]
async fn test_single_command_mode_bounds_a_second_process_or_refuses_it() {
    if Confinement::probe(ConfinementMode::SingleCommand)
        .await
        .is_err()
    {
        return;
    }

    let workspace = workspace();
    write_forking_scripts(
        &workspace.root,
        &format!("cat {} > child.marker\n", workspace.denied.display()),
    );

    let messages = run_confined(&InboundMessage::RunStart {
        job_id: "single-second-process".to_string(),
        workspace: workspace.root.clone(),
        command: SHELL.to_string(),
        args: vec![PARENT_SCRIPT.to_string()],
        env: HashMap::new(),
        timeout_ms: Some(20000),
        max_output_bytes: None,
        working_dir: None,
        confinement: Some(Box::new(request(&workspace.root))),
    })
    .await;

    if HOST_BACKEND.is_some_and(Backend::enforces_single_process) {
        assert_ne!(exit_code(&messages), Some(0), "{messages:?}");
        assert_eq!(
            marker(&workspace.root, CHILD_IDENTIFIER),
            None,
            "single-command mode let its command start a second process"
        );
    } else {
        assert!(
            marker(&workspace.root, CHILD_IDENTIFIER).is_some(),
            "the child never ran, so this proves nothing about its confinement: {messages:?}"
        );
    }

    // Whichever way the second process went, it read nothing from outside.
    assert_ne!(
        marker(&workspace.root, "child.marker").as_deref(),
        Some(SECRET.trim()),
        "a second process read a file outside the sandbox"
    );
}

#[tokio::test]
async fn test_a_confined_tree_cannot_execute_outside_its_execute_roots() {
    if Confinement::probe(ConfinementMode::ProcessTree)
        .await
        .is_err()
    {
        return;
    }
    // The host's backend, not seatbelt's: bubblewrap bounds a tree by its mount
    // namespace and has no exec filter, so a planted file inside a bound root
    // does run there. Asking seatbelt lets this run on a Linux host with
    // bubblewrap installed, where the refusal below cannot hold.
    if !HOST_BACKEND.is_some_and(Backend::enforces_execute_roots) {
        return;
    }

    let workspace = workspace();
    // A shebang script, not a copy of a system binary: macOS kills a copied
    // system binary for its lost code signature, which would make this test
    // pass without the sandbox refusing anything.
    let planted = workspace.root.join("planted.sh");
    fs::write(&planted, format!("#!{SHELL}\nexit 0\n")).unwrap();
    fs::set_permissions(&planted, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        workspace.root.join(PARENT_SCRIPT),
        "./planted.sh && printf 'ran' > planted.marker\n",
    )
    .unwrap();

    let granted = run_confined(&confined_tree_run(
        "tree-exec-granted",
        &workspace.root,
        PARENT_SCRIPT,
        vec![PathBuf::from(SHELL_DIRECTORY), workspace.root.clone()],
    ))
    .await;
    assert_eq!(
        exit_code(&granted),
        Some(0),
        "the planted executable must run when its directory is an execute root, or the refusal below proves nothing: {granted:?}"
    );
    assert_eq!(
        marker(&workspace.root, "planted.marker").as_deref(),
        Some("ran")
    );
    fs::remove_file(workspace.root.join("planted.marker")).unwrap();

    let refused = run_confined(&confined_tree_run(
        "tree-exec-bound",
        &workspace.root,
        PARENT_SCRIPT,
        vec![PathBuf::from(SHELL_DIRECTORY)],
    ))
    .await;

    assert_ne!(exit_code(&refused), Some(0), "{refused:?}");
    assert_eq!(
        marker(&workspace.root, "planted.marker"),
        None,
        "a tree executed a file it had written into its own workspace"
    );
}

/// The tree-mode counterpart of the single-command network test: the process
/// that reaches for the listener is a forked descendant.
#[tokio::test]
async fn test_a_confined_tree_blocks_a_grandchild_connection_that_otherwise_succeeds() {
    if Confinement::probe(ConfinementMode::ProcessTree)
        .await
        .is_err()
    {
        return;
    }
    let Some(client) = ["/usr/bin/nc", "/bin/nc", "/usr/bin/curl"]
        .into_iter()
        .find(|candidate| Path::new(candidate).is_file())
    else {
        return;
    };
    let client_directory = Path::new(client).parent().unwrap().to_path_buf();

    for confined in [false, true] {
        let workspace = workspace();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let connected = Arc::new(AtomicBool::new(false));
        let accepted = connected.clone();
        let acceptor = tokio::spawn(async move {
            if listener.accept().await.is_ok() {
                accepted.store(true, Ordering::SeqCst);
            }
        });

        let attempt = if client.ends_with("curl") {
            format!("{client} --silent --max-time 2 http://127.0.0.1:{port}/\n")
        } else {
            format!("{client} -w 1 127.0.0.1 {port} < /dev/null\n")
        };
        write_forking_scripts(&workspace.root, &attempt);

        let mut request = confined_tree_run(
            "tree-network",
            &workspace.root,
            PARENT_SCRIPT,
            vec![PathBuf::from(SHELL_DIRECTORY), client_directory.clone()],
        );
        if let InboundMessage::RunStart {
            confinement: slot, ..
        } = &mut request
            && !confined
        {
            *slot = None;
        }

        let messages = run_confined(&request).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        acceptor.abort();

        let parent = marker(&workspace.root, PARENT_IDENTIFIER);
        let child = marker(&workspace.root, CHILD_IDENTIFIER);
        assert!(
            parent.is_some() && child.is_some() && parent != child,
            "the grandchild never ran, so this case proves nothing (confined={confined})"
        );

        if confined {
            assert!(
                !connected.load(Ordering::SeqCst),
                "a forked grandchild of a confined tree reached a local TCP listener"
            );
            assert_ne!(exit_code(&messages), Some(0));
        } else {
            assert!(
                connected.load(Ordering::SeqCst),
                "the unconfined control never reached the listener, so this test proves nothing"
            );
        }
    }
}

#[tokio::test]
async fn test_spawn_fails_closed_when_a_tree_cannot_be_bounded() {
    let workspace = workspace();
    let (tx, mut rx) = mpsc::channel(100);

    let request = confined_tree_run("unbounded", &workspace.root, PARENT_SCRIPT, vec![]);
    let result = CommandExecutor::new().spawn(&request, tx).await;

    match result {
        Err(error @ ExecutorError::ConfinementUnavailable(_)) => {
            assert_eq!(error.to_error_code(), ErrorCode::ConfinementUnavailable);
        }
        Err(error) => panic!("Wrong error type: {error:?}"),
        Ok(_) => panic!("An unbounded tree must not spawn the command"),
    }

    assert!(
        collect_messages(&mut rx, Duration::from_millis(200))
            .await
            .is_empty(),
        "A refused spawn must not report a started process"
    );
}
