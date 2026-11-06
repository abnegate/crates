use std::fs;
use std::fs::DirBuilder;
use std::fs::File;
use std::fs::OpenOptions;
use std::fs::Permissions;
use std::fs::TryLockError;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;

use nix::fcntl::OFlag;

use crate::login::LoginError;
use crate::login::codex::Account;
use crate::login::codex::CREDENTIALS;
use crate::login::codex::signed_in;

/// The directory under a root that device sign-ins stage in.
pub(crate) const DIRECTORY: &str = ".login";

const MODE: u32 = 0o700;
const PREFIX: &str = "attempt-";

/// The private directory a device sign-in ran codex in, holding the login
/// codex saved once it succeeded.
///
/// Codex deletes the login in its `CODEX_HOME` before it prints a code, so
/// it never runs in a login's home: a refused, cancelled or expired attempt
/// leaves every login where it was, and which home the new login belongs in
/// is known only once codex has saved it. Promote it into that home, or
/// read its `auth.json` from [`path`](Self::path) and keep it however the
/// caller keeps credentials. It holds a lock on its directory for as long
/// as it lives, so no other sign-in under the same root clears it. Dropping
/// it removes the directory and what is left in it.
#[derive(Debug)]
pub struct Staging {
    path: PathBuf,
    _lock: File,
}

impl Staging {
    /// A fresh, private directory under `<root>/.login`, once whatever an
    /// earlier attempt that no longer runs left there has been removed.
    /// Attempts that are still running, or whose login waits to be promoted,
    /// are left alone.
    pub(crate) fn create(root: &Path) -> Result<Self, LoginError> {
        let parent = root.join(DIRECTORY);
        let prepare = |error: io::Error| filesystem("prepare", &parent, &error);
        let _guard = locked(&parent).map_err(prepare)?;
        sweep(&parent).map_err(prepare)?;
        let path = tempfile::Builder::new()
            .prefix(PREFIX)
            .permissions(Permissions::from_mode(MODE))
            .tempdir_in(&parent)
            .map_err(prepare)?
            .keep();
        let lock = match claim(&path) {
            Ok(Some(lock)) => lock,
            Ok(None) => {
                let _ = remove(&path);
                return Err(prepare(io::Error::from(io::ErrorKind::WouldBlock)));
            }
            Err(error) => {
                let _ = remove(&path);
                return Err(prepare(error));
            }
        };
        let staging = Self { path, _lock: lock };
        fs::set_permissions(&staging.path, Permissions::from_mode(MODE))
            .map_err(|error| filesystem("prepare", &staging.path, &error))?;
        Ok(staging)
    }

    /// Where codex ran, and where it saved its `auth.json`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The account of the login codex saved, when it names one.
    pub fn account(&self) -> Option<Account> {
        Account::read(&self.path)
    }

    /// This directory, once codex has saved a login in it.
    pub(crate) fn saved(self) -> Result<Self, LoginError> {
        if signed_in(&self.path) {
            Ok(self)
        } else {
            Err(LoginError::Failed {
                reason: "codex reported a sign-in but saved no login".to_string(),
            })
        }
    }

    /// Moves the login codex saved into `into`, a login's home on the same
    /// filesystem, over any login already there, in one rename.
    ///
    /// # Errors
    ///
    /// [`LoginError::Filesystem`] when the login cannot be moved, such as
    /// when `into` does not exist, is on another filesystem, or holds a
    /// directory where the login goes.
    pub fn promote(&self, into: &Path) -> Result<(), LoginError> {
        let credentials = into.join(CREDENTIALS);
        fs::rename(self.path.join(CREDENTIALS), &credentials)
            .map_err(|error| filesystem("save the new login as", &credentials, &error))
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if let Err(error) = remove(&self.path) {
            tracing::warn!(
                path = %self.path.display(),
                %error,
                "could not remove codex's sign-in staging directory"
            );
        }
        if let Some(parent) = self.path.parent()
            && let Ok(Some(_guard)) = held(parent)
        {
            let _ = fs::remove_dir(parent);
        }
    }
}

fn filesystem(action: &'static str, path: &Path, error: &io::Error) -> LoginError {
    LoginError::Filesystem {
        action,
        path: path.to_path_buf(),
        reason: error.to_string(),
    }
}

/// `parent`, created private when it is missing, locked until the returned
/// handle is dropped, so that no attempt is created, cleared or removed
/// under it meanwhile.
fn locked(parent: &Path) -> io::Result<File> {
    loop {
        match fs::symlink_metadata(parent) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => remove(parent)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        match DirBuilder::new().mode(MODE).create(parent) {
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
        if let Some(guard) = held(parent)? {
            fs::set_permissions(parent, Permissions::from_mode(MODE))?;
            return Ok(guard);
        }
    }
}

/// `parent` locked, once whoever holds it lets go, or `None` when it was
/// removed or replaced before the lock was taken.
fn held(parent: &Path) -> io::Result<Option<File>> {
    let directory = match open(parent) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    directory.lock()?;
    let opened = directory.metadata()?;
    match fs::symlink_metadata(parent) {
        Ok(current) if current.dev() == opened.dev() && current.ino() == opened.ino() => {
            Ok(Some(directory))
        }
        Ok(_) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Removes everything under `parent` but the attempts still held. An
/// attempt that removes itself meanwhile is gone either way.
fn sweep(parent: &Path) -> io::Result<()> {
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let path = entry.path();
        let cleared = match entry.file_type() {
            Ok(kind) if !kind.is_dir() => fs::remove_file(&path),
            Ok(_) => claim(&path).and_then(|lock| match lock {
                Some(_lock) => fs::remove_dir_all(&path),
                None => Ok(()),
            }),
            Err(error) => Err(error),
        };
        match cleared {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

/// The lock on the attempt directory at `path`, or `None` while another
/// attempt holds it.
fn claim(path: &Path) -> io::Result<Option<File>> {
    let directory = open(path)?;
    match directory.try_lock() {
        Ok(()) => Ok(Some(directory)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// The directory at `path`, never through a link.
fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW).bits())
        .open(path)
}

/// Removes `path` whatever it is, without following it if it is a link.
fn remove(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    const LOCKED: u32 = 0o500;

    #[test]
    fn a_staging_directory_that_cannot_be_cleared_is_a_filesystem_failure() {
        let root = TempDir::new().expect("an agent's root");
        let leftover = root.path().join(DIRECTORY).join("leftover");
        fs::create_dir_all(&leftover).expect("a directory an earlier attempt left");
        fs::write(leftover.join("file"), b"").expect("a file an earlier attempt left");
        fs::set_permissions(&leftover, Permissions::from_mode(LOCKED))
            .expect("the leftover locked");

        let staging = Staging::create(root.path());
        fs::set_permissions(&leftover, Permissions::from_mode(MODE))
            .expect("the leftover unlocked");

        let error = staging.err();
        assert!(
            matches!(error, Some(LoginError::Filesystem { .. })),
            "{error:?}"
        );
    }

    #[test]
    fn an_attempt_stages_under_the_root_in_a_private_directory_of_its_own() {
        let root = TempDir::new().expect("an agent's root");
        let stale = root.path().join(DIRECTORY).join("attempt-crashed");
        fs::create_dir_all(&stale).expect("a directory a crashed attempt left");

        let staging = Staging::create(root.path()).expect("a staging directory");

        assert_eq!(
            staging.path().parent(),
            Some(root.path().join(DIRECTORY).as_path())
        );
        let mode = fs::metadata(staging.path())
            .expect("the staging directory")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, MODE);
        let parent = fs::metadata(root.path().join(DIRECTORY))
            .expect("the staging root")
            .permissions()
            .mode();
        assert_eq!(parent & 0o777, MODE);
        assert!(!stale.exists(), "a crashed attempt's directory was kept");
        drop(staging);
        assert!(
            !root.path().join(DIRECTORY).exists(),
            "the staging directory was left"
        );
    }

    #[test]
    fn a_new_attempt_leaves_one_still_held_under_the_same_root() {
        let root = TempDir::new().expect("an agent's root");
        let first = Staging::create(root.path()).expect("the first staging directory");
        fs::write(first.path().join(CREDENTIALS), b"{}").expect("the login codex saved");

        let second = Staging::create(root.path()).expect("the second staging directory");

        assert_ne!(first.path(), second.path());
        let first = first
            .saved()
            .expect("the first login, still waiting to be promoted");
        drop(second);
        assert!(first.path().exists(), "the held attempt was removed");
        drop(first);
        assert!(
            !root.path().join(DIRECTORY).exists(),
            "the staging directory was left"
        );
    }

    #[test]
    fn overlapping_attempts_under_one_root_never_fail_one_another() {
        const ATTEMPTS: usize = 8;
        const ROUNDS: usize = 50;

        let root = TempDir::new().expect("an agent's root");
        std::thread::scope(|scope| {
            for _ in 0..ATTEMPTS {
                scope.spawn(|| {
                    for _ in 0..ROUNDS {
                        let staging = Staging::create(root.path()).expect("a staging directory");
                        fs::write(staging.path().join(CREDENTIALS), b"{}")
                            .expect("the login codex saved");
                        staging.saved().expect("the login, until it is dropped");
                    }
                });
            }
        });

        assert!(
            !root.path().join(DIRECTORY).exists(),
            "the staging directory was left"
        );
    }

    #[test]
    fn a_saved_login_is_promoted_into_the_home_it_is_given() {
        let root = TempDir::new().expect("an agent's root");
        let home = root.path().join("logins").join("one");
        fs::create_dir_all(&home).expect("a login's home");
        fs::write(home.join(CREDENTIALS), b"old").expect("the login it replaces");
        let staging = Staging::create(root.path()).expect("a staging directory");
        fs::write(staging.path().join(CREDENTIALS), b"new").expect("the login codex saved");

        let staging = staging.saved().expect("a saved login");
        staging.promote(&home).expect("the login promoted");

        assert_eq!(
            fs::read(home.join(CREDENTIALS)).expect("the promoted login"),
            b"new"
        );
        assert!(!signed_in(staging.path()));
    }

    #[test]
    fn a_staging_directory_codex_saved_nothing_in_is_no_sign_in() {
        let root = TempDir::new().expect("an agent's root");
        let staging = Staging::create(root.path()).expect("a staging directory");

        let saved = staging.saved();

        assert!(matches!(saved, Err(LoginError::Failed { .. })), "{saved:?}");
    }

    #[test]
    fn a_login_that_cannot_be_moved_into_its_home_is_a_filesystem_failure() {
        let root = TempDir::new().expect("an agent's root");
        let home = root.path().join("home");
        let staging = Staging::create(root.path()).expect("a staging directory");
        fs::write(staging.path().join(CREDENTIALS), b"{}").expect("the login codex saved");
        let occupied = home.join(CREDENTIALS);
        fs::create_dir_all(&occupied).expect("a directory where the login goes");
        fs::write(occupied.join("file"), b"").expect("a file inside it");

        let promoted = staging.promote(&home);

        assert!(
            matches!(promoted, Err(LoginError::Filesystem { .. })),
            "{promoted:?}"
        );
    }
}
