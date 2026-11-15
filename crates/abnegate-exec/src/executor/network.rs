#[cfg(all(
    target_os = "linux",
    target_endian = "little",
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )
))]
mod denial;
#[cfg(any(
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
))]
mod descriptors;
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
use std::io;

use tokio::process::Command;

/// Whether an unconfined command, and every process it starts in turn, may
/// reach a network.
///
/// [`Denied`](Network::Denied) is enforced by the operating system rather than
/// by inspecting what a command runs, so an interpreter, a shell, or a program
/// the command starts is denied just the same: on macOS the command runs
/// under a seatbelt profile that allows everything but the network,
/// LaunchServices and sending Apple events, and on Linux (x86-64, AArch64 and
/// RISC-V 64) under a seccomp filter that refuses every socket but a netlink
/// one. A host that can enforce neither fails to start a denied command
/// rather than running it with a network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Network {
    /// The command reaches whatever network the executor itself can.
    #[default]
    Open,
    /// The command, and everything it starts, reaches no network: not the
    /// internet, not a name server, not a listener on loopback, and not a
    /// named or abstract Unix socket, through which a local daemon could
    /// reach one for it. Pipes and stream socket pairs still work, so a
    /// pipeline, or a channel between the command and what it starts, is
    /// unaffected.
    ///
    /// The command inherits no descriptor beyond its standard input, output
    /// and error, so a socket the executor holds, already connected or not,
    /// does not reach it, unless a `pre_exec` hook added to the command makes
    /// one inheritable again.
    ///
    /// On Linux the command, and everything it starts, also:
    ///
    /// - makes no datagram socket pair, which could address a named socket;
    /// - runs with `no_new_privs`, so a setuid program or one with file
    ///   capabilities, such as `sudo`, `ping` or `fusermount`, gains no
    ///   privilege in it;
    /// - traces no process outside it, the executor included, and reaches
    ///   none of their memory, environment or descriptors through `/proc`,
    ///   so it can neither make one reach the network for it nor reopen a
    ///   pipe one holds, such as an MCP server's standard input. A Landlock
    ///   domain that refuses no file access keeps it in, and a host without
    ///   Landlock fails to spawn the command with `EPERM`.
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

    /// `error`, from spawning a [`command`](Network::command) with this
    /// network access, with the reason a denied command most likely failed
    /// to start appended.
    ///
    /// The denial is put in place from a `pre_exec` hook, and the standard
    /// library hands back only the hook's error number, so a host that cannot
    /// deny the network reports a bare "Operation not permitted" or "Invalid
    /// argument". An open command's error, and a missing program's
    /// [`io::ErrorKind::NotFound`], are returned unchanged.
    pub(crate) fn spawn_error(self, error: io::Error) -> io::Error {
        match self {
            Network::Denied if error.kind() != io::ErrorKind::NotFound => {
                io::Error::new(error.kind(), format!("{error}: {UNDENIABLE}"))
            }
            _ => error,
        }
    }
}

/// Why a denied command that failed to start most likely did.
const UNDENIABLE: &str =
    "the command did not start, likely because this host cannot keep it off the network";

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
    denial::deny(&mut command);
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
    use std::os::fd::AsRawFd;
    use std::os::fd::FromRawFd;
    use std::os::fd::OwnedFd;
    use std::os::fd::RawFd;
    use std::os::unix::net::UnixDatagram;
    use std::os::unix::net::UnixListener;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::process::Stdio;

    use nix::libc;
    use tempfile::TempDir;

    use crate::testing::network_denial_enforced as enforced;

    use super::*;

    /// Names the loopback address a [`network_probe`] child tries to reach.
    const PROBE_TARGET: &str = "ABNEGATE_EXEC_NETWORK_PROBE";

    /// Names the path of a Unix stream listener the probe tries to connect to.
    const PROBE_NAMED_UNIX: &str = "ABNEGATE_EXEC_NETWORK_PROBE_NAMED_UNIX";

    /// Names the path of a bound Unix datagram socket the probe tries to send
    /// to.
    const PROBE_DATAGRAM: &str = "ABNEGATE_EXEC_NETWORK_PROBE_DATAGRAM";

    /// Names the abstract Unix listener the probe tries to connect to.
    #[cfg(target_os = "linux")]
    const PROBE_ABSTRACT_UNIX: &str = "ABNEGATE_EXEC_NETWORK_PROBE_ABSTRACT_UNIX";

    /// Names the process whose memory, and whose pipe, the probe tries to
    /// open for writing.
    #[cfg(target_os = "linux")]
    const PROBE_PARENT: &str = "ABNEGATE_EXEC_NETWORK_PROBE_PARENT";

    /// Names the write end of a pipe the test process holds, which the probe
    /// tries to reopen through `/proc`.
    #[cfg(target_os = "linux")]
    const PROBE_PIPE: &str = "ABNEGATE_EXEC_NETWORK_PROBE_PIPE";

    /// Names an inheritable descriptor the test process holds, which the probe
    /// checks it did not inherit.
    const PROBE_INHERITED: &str = "ABNEGATE_EXEC_NETWORK_PROBE_INHERITED";

    /// The lowest number the inheritable descriptor is duplicated to, well
    /// clear of any the probe opens itself.
    const INHERITED_FLOOR: RawFd = 200;

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

    /// `ok`, or the error number the last call failed with, for a raw call
    /// that answered `-1`.
    fn errno(result: libc::c_long) -> String {
        match result {
            -1 => io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or_default()
                .to_string(),
            _ => "ok".to_string(),
        }
    }

    fn variable(name: &str) -> String {
        std::env::var(name).unwrap_or_else(|_| panic!("the probe was given no {name}"))
    }

    #[cfg(target_os = "linux")]
    fn io_uring() -> String {
        // A null parameter block: the kernel answers EFAULT when it lets the
        // call through at all, so no ring is ever made.
        #[allow(unsafe_code)]
        let result =
            unsafe { libc::syscall(libc::SYS_io_uring_setup, 1, std::ptr::null_mut::<u8>()) };
        errno(result)
    }

    fn inherited() -> String {
        let descriptor: RawFd = variable(PROBE_INHERITED)
            .parse()
            .expect("an inherited descriptor number");
        #[allow(unsafe_code)]
        let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
        errno(flags.into())
    }

    fn datagram() -> io::Result<usize> {
        UnixDatagram::unbound()?.send_to(b"probe", variable(PROBE_DATAGRAM))
    }

    #[cfg(target_os = "linux")]
    fn abstract_unix() -> io::Result<UnixStream> {
        use std::os::linux::net::SocketAddrExt;
        use std::os::unix::net::SocketAddr;

        let address = SocketAddr::from_abstract_name(variable(PROBE_ABSTRACT_UNIX))?;
        UnixStream::connect_addr(&address)
    }

    #[cfg(target_os = "linux")]
    fn memory() -> io::Result<std::fs::File> {
        let parent = variable(PROBE_PARENT);
        std::fs::OpenOptions::new()
            .write(true)
            .open(format!("/proc/{parent}/mem"))
    }

    #[cfg(target_os = "linux")]
    fn proc_fd() -> io::Result<std::fs::File> {
        let parent = variable(PROBE_PARENT);
        let pipe = variable(PROBE_PIPE);
        std::fs::OpenOptions::new()
            .write(true)
            .open(format!("/proc/{parent}/fd/{pipe}"))
    }

    /// Not a test in itself: run as a child with [`PROBE_TARGET`] set, it
    /// prints one `name=outcome` line for each way it tries to reach a
    /// network, or something that could reach one for it, and one for a Unix
    /// stream socket pair, which must always work.
    #[test]
    fn network_probe() {
        let Some(target) = std::env::var_os(PROBE_TARGET) else {
            return;
        };
        println!("inherited={}", inherited());
        let target = target.to_string_lossy().into_owned();
        println!("tcp={}", outcome(TcpStream::connect(&target)));
        println!("udp={}", outcome(UdpSocket::bind("0.0.0.0:0")));
        println!("ipv6={}", outcome(UdpSocket::bind("[::]:0")));
        println!("unix={}", outcome(UnixStream::pair()));
        println!(
            "named_unix={}",
            outcome(UnixStream::connect(variable(PROBE_NAMED_UNIX)))
        );
        println!("datagram={}", outcome(datagram()));
        #[cfg(target_os = "linux")]
        {
            println!("datagram_pair={}", outcome(UnixDatagram::pair()));
            println!("abstract_unix={}", outcome(abstract_unix()));
            println!("memory={}", outcome(memory()));
            println!("proc_fd={}", outcome(proc_fd()));
            println!("io_uring={}", io_uring());
        }
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

        fn assert(&self, name: &str, expected: &str) {
            assert_eq!(self.get(name), expected, "{name}:\n{}", self.stdout);
        }
    }

    fn executable() -> PathBuf {
        std::env::current_exe().expect("the test binary's path")
    }

    /// Everything a probe tries to reach, live for as long as the probe runs:
    /// a loopback listener, a named Unix listener and datagram socket, an
    /// inheritable descriptor, and on Linux an abstract Unix listener and a
    /// pipe.
    struct Targets {
        tcp: TcpListener,
        _directory: TempDir,
        named_unix: PathBuf,
        _named_listener: UnixListener,
        datagram: PathBuf,
        _datagram_socket: UnixDatagram,
        #[cfg(target_os = "linux")]
        abstract_name: String,
        #[cfg(target_os = "linux")]
        _abstract_listener: UnixListener,
        #[cfg(target_os = "linux")]
        pipe: (io::PipeReader, io::PipeWriter),
        inherited: OwnedFd,
    }

    impl Targets {
        fn new() -> Self {
            let tcp = TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
            let directory = tempfile::tempdir().expect("a directory for Unix sockets");
            let named_unix = directory.path().join("stream");
            let named_listener = UnixListener::bind(&named_unix).expect("a named Unix listener");
            let datagram = directory.path().join("datagram");
            let datagram_socket = UnixDatagram::bind(&datagram).expect("a named datagram socket");
            #[cfg(target_os = "linux")]
            let (abstract_name, abstract_listener) = abstract_listener();
            let inherited = inheritable(&tcp);
            #[cfg(target_os = "linux")]
            let pipe = io::pipe().expect("a pipe");
            Self {
                tcp,
                _directory: directory,
                named_unix,
                _named_listener: named_listener,
                datagram,
                _datagram_socket: datagram_socket,
                #[cfg(target_os = "linux")]
                abstract_name,
                #[cfg(target_os = "linux")]
                _abstract_listener: abstract_listener,
                #[cfg(target_os = "linux")]
                pipe,
                inherited,
            }
        }

        fn apply(&self, command: &mut Command) {
            let address = self.tcp.local_addr().expect("the listener's address");
            command
                .env(PROBE_TARGET, address.to_string())
                .env(PROBE_NAMED_UNIX, &self.named_unix)
                .env(PROBE_DATAGRAM, &self.datagram)
                .env(PROBE_INHERITED, self.inherited.as_raw_fd().to_string());
            #[cfg(target_os = "linux")]
            command
                .env(PROBE_ABSTRACT_UNIX, &self.abstract_name)
                .env(PROBE_PARENT, std::process::id().to_string())
                .env(PROBE_PIPE, self.pipe.1.as_raw_fd().to_string());
        }
    }

    #[cfg(target_os = "linux")]
    fn abstract_listener() -> (String, UnixListener) {
        use std::os::linux::net::SocketAddrExt;
        use std::os::unix::net::SocketAddr;

        let name = format!("abnegate-exec-probe-{}", uuid::Uuid::new_v4());
        let address = SocketAddr::from_abstract_name(&name).expect("an abstract address");
        let listener = UnixListener::bind_addr(&address).expect("an abstract Unix listener");
        (name, listener)
    }

    /// A duplicate of `socket` that every child the test process spawns
    /// inherits, unless something closes it on the way.
    fn inheritable(socket: &TcpListener) -> OwnedFd {
        #[allow(unsafe_code)]
        let duplicate = unsafe { libc::fcntl(socket.as_raw_fd(), libc::F_DUPFD, INHERITED_FLOOR) };
        assert_ne!(
            duplicate,
            -1,
            "an inheritable duplicate: {}",
            io::Error::last_os_error()
        );
        // The descriptor was just made, and nothing else owns it.
        #[allow(unsafe_code)]
        unsafe {
            OwnedFd::from_raw_fd(duplicate)
        }
    }

    async fn report(mut command: Command, targets: &Targets) -> Report {
        targets.apply(&mut command);
        let output = command
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

    async fn probe(network: Network, targets: &Targets) -> Report {
        let mut command = network.command(executable());
        command.args(["--exact", PROBE, "--nocapture"]);
        report(command, targets).await
    }

    /// Whether Yama lets a process write the memory of its ancestor, as an
    /// open command's probe tries to.
    #[cfg(target_os = "linux")]
    fn ancestors_traceable() -> bool {
        std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope")
            .map_or(true, |scope| matches!(scope.trim(), "0" | "1"))
    }

    fn assert_denied(report: &Report) {
        for name in ["tcp", "udp", "ipv6", "named_unix", "datagram"] {
            report.assert(name, "PermissionDenied");
        }
        report.assert("unix", "ok");
        report.assert("inherited", &libc::EBADF.to_string());
        #[cfg(target_os = "linux")]
        for name in ["datagram_pair", "abstract_unix", "memory", "proc_fd"] {
            report.assert(name, "PermissionDenied");
        }
    }

    #[test]
    fn a_default_network_is_open() {
        assert_eq!(Network::default(), Network::Open);
    }

    #[test]
    fn a_denied_spawn_error_says_why_the_command_did_not_start() {
        let error = Network::Denied.spawn_error(io::Error::from_raw_os_error(libc::EPERM));

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(
            error.to_string(),
            format!(
                "{}: the command did not start, likely because this host cannot keep it off the network",
                io::Error::from_raw_os_error(libc::EPERM)
            )
        );
    }

    #[test]
    fn a_denied_spawn_error_for_a_missing_program_is_unchanged() {
        let error = Network::Denied.spawn_error(io::Error::from_raw_os_error(libc::ENOENT));

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(error.raw_os_error(), Some(libc::ENOENT));
    }

    #[test]
    fn an_open_spawn_error_is_unchanged() {
        let error = Network::Open.spawn_error(io::Error::from_raw_os_error(libc::EPERM));

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    }

    #[tokio::test]
    async fn an_open_command_reaches_every_target() {
        let targets = Targets::new();

        let report = probe(Network::Open, &targets).await;

        for name in ["inherited", "tcp", "udp", "unix", "named_unix", "datagram"] {
            report.assert(name, "ok");
        }
        #[cfg(target_os = "linux")]
        {
            report.assert("datagram_pair", "ok");
            report.assert("abstract_unix", "ok");
            report.assert("proc_fd", "ok");
            if ancestors_traceable() {
                report.assert("memory", "ok");
            } else {
                eprintln!(
                    "skipped the memory check: Yama keeps an open command from writing its ancestor's memory here"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_denied_command_reaches_no_network_but_keeps_stream_socket_pairs() {
        if !enforced().await {
            return;
        }
        let targets = Targets::new();

        let report = probe(Network::Denied, &targets).await;

        assert_denied(&report);
    }

    #[tokio::test]
    async fn a_grandchild_of_a_denied_command_reaches_no_network() {
        if !enforced().await {
            return;
        }
        let targets = Targets::new();
        let mut command = Network::Denied.command("sh");
        command
            .args(["-c", r#"sh -c '"$0" "$@"' "$0" "$@""#])
            .arg(executable())
            .args(["--exact", PROBE, "--nocapture"]);

        let report = report(command, &targets).await;

        assert_denied(&report);
    }

    /// A host whose kernel refuses io_uring to everyone, as
    /// `kernel.io_uring_disabled=2` or a container's own seccomp profile
    /// does, answers `EPERM` without the filter, so it says nothing about
    /// the filter and skips.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_denied_command_cannot_set_up_io_uring() {
        if !enforced().await {
            return;
        }
        let targets = Targets::new();
        let refused = libc::EPERM.to_string();

        let open = probe(Network::Open, &targets).await;
        if open.get("io_uring") == refused {
            eprintln!(
                "skipped: this host refuses io_uring to an open command too:\n{}",
                open.stdout
            );
            return;
        }
        let denied = probe(Network::Denied, &targets).await;

        denied.assert("io_uring", &refused);
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
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
        let address = listener.local_addr().expect("the listener's address");
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
