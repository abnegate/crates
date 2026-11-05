use super::*;
use std::fs::File;
use std::io::ErrorKind;

/// Lists every value `remote.origin.url` holds in the configuration on
/// standard input, each ended by a NUL, following no include it names.
const ORIGIN_LISTING: [&str; 7] = [
    "config",
    "--no-includes",
    "-z",
    "--file",
    "-",
    "--get-all",
    ORIGIN_URL,
];

/// How `git config --get-all` says the key is not set.
const NOT_SET: i32 = 1;

impl GitService {
    /// The address the git configuration file at `file` gives `origin`, or
    /// `None` when it gives none, read as data. The file is read here and
    /// handed to a hardened git on its standard input, so no include it names
    /// is followed, no repository around it or named by the environment is
    /// consulted, and the host's system and global configuration are ignored.
    ///
    /// A link is followed. These are refused:
    ///
    /// - with [`GitError::Io`], a file that cannot be read, and anything that
    ///   is not a regular file, such as a directory, a named pipe or a device,
    ///   which is refused before it is opened;
    /// - with [`GitError::CommandFailed`], a file git cannot parse;
    /// - with [`GitError::UnsafeConfig`], an `origin` with more than one
    ///   address, which fetches from the first and pushes to every one.
    ///
    /// It blocks the calling thread until git exits, and is not bounded by
    /// [`Self::with_timeout`]: git reaches no network and reads only what it
    /// is handed. A caller that can await runs it on the blocking pool.
    pub fn configured_origin(file: &Path) -> GitResult<Option<String>> {
        let configuration = regular(file)?;
        let mut command = std::process::Command::new("git");
        harden(&mut command);
        let output = command
            .env("GIT_ALLOW_PROTOCOL", "")
            .current_dir("/")
            .args(ORIGIN_LISTING)
            .stdin(configuration)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?;
        match output.status.code() {
            Some(0) => sole_origin(&output.stdout).map(Some),
            Some(NOT_SET) => Ok(None),
            _ => Err(Self::failed("config", &output)),
        }
    }
}

/// The file at `file`, opened for git to read as its standard input, or a
/// refusal when it is not a regular file.
///
/// It is examined before it is opened, because opening a device can act on
/// it: a terminal opened by a process with none becomes its controlling one.
/// It is checked again once open, and must still be the file examined, so
/// what is checked is what git reads.
fn regular(file: &Path) -> std::io::Result<File> {
    let examined = std::fs::metadata(file)?;
    if !examined.is_file() {
        return Err(not_regular());
    }
    let opened = opened(file)?;
    let metadata = opened.metadata()?;
    if !metadata.is_file() || !same_file(&examined, &metadata) {
        return Err(not_regular());
    }
    Ok(opened)
}

fn not_regular() -> std::io::Error {
    std::io::Error::new(
        ErrorKind::InvalidInput,
        "The git configuration is not a regular file",
    )
}

#[cfg(unix)]
fn same_file(examined: &std::fs::Metadata, opened: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    examined.dev() == opened.dev() && examined.ino() == opened.ino()
}

#[cfg(not(unix))]
fn same_file(_examined: &std::fs::Metadata, _opened: &std::fs::Metadata) -> bool {
    true
}

#[cfg(unix)]
fn opened(file: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags((nix::fcntl::OFlag::O_NONBLOCK | nix::fcntl::OFlag::O_NOCTTY).bits())
        .open(file)
}

#[cfg(not(unix))]
fn opened(file: &Path) -> std::io::Result<File> {
    File::open(file)
}

/// The one address an [`ORIGIN_LISTING`] holds.
fn sole_origin(listing: &[u8]) -> GitResult<String> {
    let unreadable = || GitError::CommandFailed("Cannot read the configured origin".to_string());
    let value = listing.strip_suffix(b"\0").ok_or_else(unreadable)?;
    if value.contains(&0) {
        return Err(GitError::UnsafeConfig(ORIGIN_URL.to_string()));
    }
    String::from_utf8(value.to_vec()).map_err(|_| unreadable())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::fixtures::git;
    use tempfile::TempDir;

    /// What a test run again in a child process finds set, naming the test.
    const CHILD: &str = "ABNEGATE_VCS_TEST_CHILD";

    const ADDRESS: &str = "https://github.com/owner/repository.git";

    const ELSEWHERE: &str = "https://elsewhere.test/planted.git";

    fn configuration(directory: &TempDir, name: &str, content: &str) -> PathBuf {
        let file = directory.path().join(name);
        std::fs::write(&file, content).unwrap();
        file
    }

    fn origin(address: &str) -> String {
        format!("[remote \"origin\"]\n\turl = {address}\n")
    }

    fn upstream_only() -> String {
        format!("[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = {ELSEWHERE}\n")
    }

    #[test]
    fn the_origin_a_configuration_names_is_read() {
        let directory = TempDir::new().unwrap();
        let file = configuration(
            &directory,
            "config",
            &format!("{}{}", upstream_only(), origin(ADDRESS)),
        );

        let read = GitService::configured_origin(&file).unwrap();

        assert_eq!(read.as_deref(), Some(ADDRESS));
    }

    #[test]
    fn a_configuration_without_an_origin_names_none() {
        let directory = TempDir::new().unwrap();
        let file = configuration(&directory, "config", &upstream_only());

        let read = GitService::configured_origin(&file).unwrap();

        assert_eq!(read, None);
    }

    #[test]
    fn an_include_that_sets_the_origin_is_never_followed() {
        let directory = TempDir::new().unwrap();
        let included = configuration(&directory, "included", &origin(ELSEWHERE));
        let file = configuration(
            &directory,
            "config",
            &format!(
                "{}[include]\n\tpath = {}\n",
                upstream_only(),
                included.display()
            ),
        );

        let read = GitService::configured_origin(&file).unwrap();

        assert_eq!(read, None, "an include was followed");
    }

    #[test]
    fn an_origin_with_more_than_one_address_is_refused() {
        let directory = TempDir::new().unwrap();
        let file = configuration(
            &directory,
            "config",
            &format!("{}\turl = {ADDRESS}\n", origin(ELSEWHERE)),
        );

        let refusal = GitService::configured_origin(&file);

        assert!(
            matches!(&refusal, Err(GitError::UnsafeConfig(key)) if key == ORIGIN_URL),
            "{refusal:?}"
        );
    }

    #[test]
    fn a_configuration_git_cannot_parse_is_refused() {
        let directory = TempDir::new().unwrap();
        let file = configuration(&directory, "config", "[remote \"origin\"\n\turl =");

        let refusal = GitService::configured_origin(&file);

        assert!(
            matches!(refusal, Err(GitError::CommandFailed(_))),
            "{refusal:?}"
        );
    }

    #[test]
    fn a_configuration_that_is_not_there_is_refused() {
        let directory = TempDir::new().unwrap();

        let refusal = GitService::configured_origin(&directory.path().join("config"));

        assert!(
            matches!(&refusal, Err(GitError::Io(error)) if error.kind() == ErrorKind::NotFound),
            "{refusal:?}"
        );
    }

    #[test]
    fn a_directory_is_refused_rather_than_read_as_naming_no_origin() {
        let directory = TempDir::new().unwrap();

        let refusal = GitService::configured_origin(directory.path());

        assert!(
            matches!(&refusal, Err(GitError::Io(error)) if error.kind() == ErrorKind::InvalidInput),
            "{refusal:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_file_swapped_after_it_was_examined_is_not_the_same_file() {
        let directory = TempDir::new().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::write(&first, "").unwrap();
        std::fs::write(&second, "").unwrap();

        let examined = std::fs::metadata(&first).unwrap();

        assert!(same_file(&examined, &std::fs::metadata(&first).unwrap()));
        assert!(!same_file(&examined, &std::fs::metadata(&second).unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_configuration_is_refused_rather_than_read_as_naming_no_origin() {
        use std::os::unix::fs::PermissionsExt;
        if nix::unistd::geteuid().is_root() {
            return;
        }
        let directory = TempDir::new().unwrap();
        let file = configuration(&directory, "config", &origin(ADDRESS));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();

        let refusal = GitService::configured_origin(&file);

        assert!(
            matches!(&refusal, Err(GitError::Io(error)) if error.kind() == ErrorKind::PermissionDenied),
            "{refusal:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_device_is_refused_rather_than_read_as_a_configuration() {
        let directory = TempDir::new().unwrap();
        let file = directory.path().join("config");
        std::os::unix::fs::symlink("/dev/null", &file).unwrap();

        let refusal = GitService::configured_origin(&file);

        assert!(
            matches!(&refusal, Err(GitError::Io(error)) if error.kind() == ErrorKind::InvalidInput),
            "{refusal:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_named_pipe_is_refused_rather_than_waited_on() {
        let directory = TempDir::new().unwrap();
        let file = directory.path().join("config");
        nix::unistd::mkfifo(&file, nix::sys::stat::Mode::S_IRWXU).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();

        std::thread::spawn(move || sender.send(GitService::configured_origin(&file)));
        let refusal = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("a named pipe was waited on");

        assert!(
            matches!(&refusal, Err(GitError::Io(error)) if error.kind() == ErrorKind::InvalidInput),
            "{refusal:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_configuration_is_followed() {
        let directory = TempDir::new().unwrap();
        let target = configuration(&directory, "target", &origin(ADDRESS));
        let file = directory.path().join("config");
        std::os::unix::fs::symlink(&target, &file).unwrap();

        let read = GitService::configured_origin(&file).unwrap();

        assert_eq!(read.as_deref(), Some(ADDRESS));
    }

    /// Every way the environment can hand git configuration or a repository
    /// names one whose origin is elsewhere: none of it is read.
    #[test]
    fn configuration_the_environment_names_is_never_read() {
        const NAME: &str =
            "git::service::origin::tests::configuration_the_environment_names_is_never_read";
        let fixture = std::env::var_os("ABNEGATE_VCS_TEST_CONFIGURATION");
        if std::env::var(CHILD).as_deref() == Ok(NAME) {
            let file = PathBuf::from(fixture.expect("the parent names the configuration"));
            assert_eq!(GitService::configured_origin(&file).unwrap(), None);
            return;
        }
        let directory = TempDir::new().unwrap();
        let file = configuration(&directory, "config", &upstream_only());
        let planted = configuration(&directory, "planted", &origin(ELSEWHERE));
        let repository = directory.path().join("repository");
        std::fs::create_dir(&repository).unwrap();
        git(&repository, &["init", "-q"]);
        git(&repository, &["config", ORIGIN_URL, ELSEWHERE]);

        let output = std::process::Command::new(std::env::current_exe().expect("the test binary"))
            .args(["--exact", NAME, "--nocapture"])
            .env(CHILD, NAME)
            .env("ABNEGATE_VCS_TEST_CONFIGURATION", &file)
            .env("GIT_DIR", repository.join(GIT_DIRECTORY))
            .env("GIT_CONFIG", &planted)
            .env("GIT_CONFIG_GLOBAL", &planted)
            .env("GIT_CONFIG_SYSTEM", &planted)
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", ORIGIN_URL)
            .env("GIT_CONFIG_VALUE_0", ELSEWHERE)
            .env(
                "GIT_CONFIG_PARAMETERS",
                format!("'{ORIGIN_URL}'='{ELSEWHERE}'"),
            )
            .output()
            .expect("the child runs");

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "the child ran no test: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
