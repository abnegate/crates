#[cfg(all(
    target_os = "linux",
    target_endian = "little",
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )
))]
mod filter;
#[cfg(target_os = "macos")]
mod seatbelt;

use std::ffi::OsStr;

use tokio::process::Command;

/// Whether an unconfined command, and every process it starts in turn, may
/// reach a network.
///
/// [`Denied`](Network::Denied) is enforced by the operating system rather than
/// by inspecting what a command runs, so an interpreter, a shell, or a program
/// the command starts is denied just the same: on macOS the command runs
/// under a seatbelt profile that allows everything but the network,
/// LaunchServices and sending Apple events, and on Linux (x86-64, AArch64 and
/// RISC-V 64) under a seccomp filter that refuses every socket but a Unix or
/// netlink one. A host that can enforce neither fails to start a denied
/// command rather than running it with a network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Network {
    /// The command reaches whatever network the executor itself can.
    #[default]
    Open,
    /// The command, and everything it starts, reaches no network: not the
    /// internet, not a name server, and not a listener on loopback. Unix
    /// sockets still work, so a pipeline or a local IPC channel is
    /// unaffected.
    ///
    /// On macOS the command also cannot open a URL or a document through
    /// LaunchServices or send an Apple event, so it cannot hand a URL to the
    /// default browser or script another application into fetching one. Other
    /// Mach and XPC services stay reachable, and one that makes requests on a
    /// caller's behalf is outside what this denies.
    Denied,
}

impl Network {
    /// A command that runs `program` with this network access.
    ///
    /// Arguments added to the returned command reach `program` itself, and
    /// `program` is looked up in the `PATH` the command is spawned with, as an
    /// open one is: a program that is not found fails the spawn with
    /// [`std::io::ErrorKind::NotFound`]. A denied command whose denial cannot
    /// be put in place fails to spawn.
    pub fn command(self, program: impl AsRef<OsStr>) -> Command {
        match self {
            Network::Open => Command::new(program),
            Network::Denied => denied(program.as_ref()),
        }
    }
}

#[cfg(target_os = "macos")]
fn denied(program: &OsStr) -> Command {
    let mut command = Command::new(program);
    seatbelt::deny(&mut command);
    command
}

#[cfg(all(
    target_os = "linux",
    target_endian = "little",
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )
))]
fn denied(program: &OsStr) -> Command {
    let mut command = Command::new(program);
    filter::deny(&mut command);
    command
}

#[cfg(not(any(
    target_os = "macos",
    all(
        target_os = "linux",
        target_endian = "little",
        any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "riscv64"
        )
    )
)))]
fn denied(program: &OsStr) -> Command {
    let mut command = Command::new(program);
    // Runs between fork and exec, and only builds an error from its kind,
    // which allocates nothing.
    #[allow(unsafe_code)]
    unsafe {
        command.pre_exec(|| Err(std::io::Error::from(std::io::ErrorKind::Unsupported)));
    }
    command
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::net::TcpListener;
    use std::net::TcpStream;
    use std::net::UdpSocket;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::process::Stdio;

    use crate::executor::ConfinementMode;
    use crate::executor::sandbox;

    use super::*;

    /// Names the loopback address a [`network_probe`] child tries to reach.
    const PROBE_TARGET: &str = "ABNEGATE_EXEC_NETWORK_PROBE";

    /// The probe's path, as `--exact` names it.
    const PROBE: &str = "executor::network::tests::network_probe";

    /// What one attempt the probe made came to: `ok`, or the kind of error
    /// that stopped it.
    fn outcome<T>(result: io::Result<T>) -> String {
        match result {
            Ok(_) => "ok".to_string(),
            Err(error) => format!("{:?}", error.kind()),
        }
    }

    #[cfg(target_os = "linux")]
    fn io_uring() -> String {
        // A null parameter block: the kernel answers EFAULT when it lets the
        // call through at all, so no ring is ever made.
        #[allow(unsafe_code)]
        let result = unsafe {
            nix::libc::syscall(nix::libc::SYS_io_uring_setup, 1, std::ptr::null_mut::<u8>())
        };
        match result {
            -1 => io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or_default()
                .to_string(),
            _ => "ok".to_string(),
        }
    }

    /// Not a test in itself: run as a child with [`PROBE_TARGET`] set, it
    /// prints one `name=outcome` line for each way it tries to reach a
    /// network, and one for a Unix socket pair, which must always work.
    #[test]
    fn network_probe() {
        let Some(target) = std::env::var_os(PROBE_TARGET) else {
            return;
        };
        let target = target.to_string_lossy().into_owned();
        println!("tcp={}", outcome(TcpStream::connect(&target)));
        println!("udp={}", outcome(UdpSocket::bind("0.0.0.0:0")));
        println!("ipv6={}", outcome(UdpSocket::bind("[::]:0")));
        println!("unix={}", outcome(UnixStream::pair()));
        #[cfg(target_os = "linux")]
        println!("io_uring={}", io_uring());
    }

    /// Everything one probe run reported, by name.
    struct Report {
        stdout: String,
    }

    impl Report {
        fn get(&self, name: &str) -> &str {
            let prefix = format!("{name}=");
            self.stdout
                .lines()
                .find_map(|line| line.strip_prefix(prefix.as_str()))
                .unwrap_or_else(|| panic!("the probe reported no {name}:\n{}", self.stdout))
        }
    }

    fn executable() -> PathBuf {
        std::env::current_exe().expect("the test binary's path")
    }

    /// A live listener on loopback, which an open command reaches, and the
    /// address a denied one must fail to.
    fn listener() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
        let address = listener.local_addr().expect("its address").to_string();
        (listener, address)
    }

    async fn report(mut command: Command, target: &str) -> Report {
        let output = command
            .env(PROBE_TARGET, target)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .expect("the probe's output");
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(
            output.status.success(),
            "the probe failed: {stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Report { stdout }
    }

    async fn probe(network: Network, target: &str) -> Report {
        let mut command = network.command(executable());
        command.args(["--exact", PROBE, "--nocapture"]);
        report(command, target).await
    }

    /// Whether a denied command runs at all here. Seatbelt cannot nest
    /// inside a stricter sandbox, so a test host that is itself sandboxed
    /// skips rather than fails, unless confinement is required.
    async fn enforced() -> bool {
        let status = Network::Denied
            .command("true")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        if matches!(status, Ok(status) if status.success()) {
            return true;
        }
        assert!(
            !sandbox::required(ConfinementMode::SingleCommand),
            "{} is set, but a denied command cannot run here: {status:?}",
            sandbox::REQUIRE_CONFINEMENT
        );
        false
    }

    fn assert_denied(report: &Report) {
        for name in ["tcp", "udp", "ipv6"] {
            assert_eq!(
                report.get(name),
                "PermissionDenied",
                "{name} was not denied:\n{}",
                report.stdout
            );
        }
        assert_eq!(report.get("unix"), "ok", "{}", report.stdout);
    }

    #[test]
    fn a_default_network_is_open() {
        assert_eq!(Network::default(), Network::Open);
    }

    #[tokio::test]
    async fn an_open_command_reaches_a_loopback_listener() {
        let (_listener, address) = listener();

        let report = probe(Network::Open, &address).await;

        assert_eq!(report.get("tcp"), "ok", "{}", report.stdout);
        assert_eq!(report.get("udp"), "ok", "{}", report.stdout);
        assert_eq!(report.get("unix"), "ok", "{}", report.stdout);
    }

    #[tokio::test]
    async fn a_denied_command_reaches_no_network_but_keeps_unix_sockets() {
        if !enforced().await {
            return;
        }
        let (_listener, address) = listener();

        let report = probe(Network::Denied, &address).await;

        assert_denied(&report);
    }

    #[tokio::test]
    async fn a_grandchild_of_a_denied_command_reaches_no_network() {
        if !enforced().await {
            return;
        }
        let (_listener, address) = listener();
        let mut command = Network::Denied.command("sh");
        command
            .args(["-c", r#"sh -c '"$0" "$@"' "$0" "$@""#])
            .arg(executable())
            .args(["--exact", PROBE, "--nocapture"]);

        let report = report(command, &address).await;

        assert_denied(&report);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_denied_command_cannot_set_up_io_uring() {
        if !enforced().await {
            return;
        }
        let (_listener, address) = listener();

        let report = probe(Network::Denied, &address).await;

        assert_eq!(
            report.get("io_uring"),
            nix::libc::EPERM.to_string(),
            "{}",
            report.stdout
        );
    }

    #[tokio::test]
    async fn a_denied_shell_still_runs_ordinary_commands() {
        if !enforced().await {
            return;
        }
        let output = Network::Denied
            .command("sh")
            .args(["-c", "echo hi"])
            .output()
            .await
            .expect("the shell's output");
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "hi\n");

        let status = Network::Denied
            .command("ls")
            .stdout(Stdio::null())
            .status()
            .await
            .expect("ls's status");
        assert!(status.success());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn a_denied_command_runs_its_own_program_under_the_profile() {
        let command = Network::Denied.command("ls");
        let standard = command.as_std();

        assert_eq!(standard.get_program(), "ls");
        assert_eq!(standard.get_args().count(), 0);
        assert_eq!(
            seatbelt::PROFILE,
            c"(version 1)(allow default)(deny network*)(deny lsopen)(deny appleevent-send)"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn a_denied_command_cannot_open_a_url_through_launch_services() {
        if !enforced().await {
            return;
        }
        let (listener, address) = listener();
        listener
            .set_nonblocking(true)
            .expect("a non-blocking listener");

        let status = Network::Denied
            .command("open")
            .args(["-g", &format!("http://{address}/")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .expect("open's status");

        assert!(!status.success(), "open handed the URL on: {status:?}");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match listener.accept() {
                Ok((_, peer)) => panic!("{peer} reached the listener on the command's behalf"),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(error) => panic!("the listener failed: {error}"),
            }
        }
    }

    #[tokio::test]
    async fn a_denied_command_whose_program_is_missing_fails_to_spawn() {
        if !enforced().await {
            return;
        }
        let program = format!("definitely-not-a-program-{}", uuid::Uuid::new_v4());

        let result = Network::Denied.command(&program).status().await;

        assert_eq!(
            result.as_ref().map_err(io::Error::kind).err(),
            Some(io::ErrorKind::NotFound),
            "{result:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn a_denied_command_whose_absolute_program_is_missing_fails_to_spawn() {
        if !enforced().await {
            return;
        }
        let directory = tempfile::tempdir().expect("a temporary directory");
        let program = directory.path().join("missing");

        let result = Network::Denied.command(&program).status().await;

        assert_eq!(
            result.as_ref().map_err(io::Error::kind).err(),
            Some(io::ErrorKind::NotFound),
            "{result:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn a_denied_command_finds_its_program_on_the_path_it_is_spawned_with() {
        use std::os::unix::fs::PermissionsExt;

        if !enforced().await {
            return;
        }
        let directory = tempfile::tempdir().expect("a temporary directory");
        let name = format!("abnegate-path-probe-{}", uuid::Uuid::new_v4());
        let script = directory.path().join(&name);
        std::fs::write(&script, "#!/bin/sh\necho found\n").expect("the script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("the script's mode");

        let mut command = Network::Denied.command(&name);
        command.env("PATH", directory.path());
        let output = command.output().await.expect("the script's output");

        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "found\n");
    }
}
