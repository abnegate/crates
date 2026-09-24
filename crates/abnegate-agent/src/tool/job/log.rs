use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::path::PathBuf;

use nix::errno::Errno;
use nix::fcntl::AtFlags;
use nix::fcntl::OFlag;
use nix::fcntl::openat;
use nix::sys::stat::Mode;
use nix::sys::stat::SFlag;
use nix::sys::stat::fstatat;
use nix::sys::stat::mkdirat;
use nix::unistd::UnlinkatFlags;
use nix::unistd::unlinkat;

use super::JOB_LOG_DIRECTORY;
use super::log_name;
use super::log_path;
use crate::Application;

const DIRECTORY_MODE: Mode = Mode::S_IRWXU;
const LOG_MODE: Mode = Mode::from_bits_truncate(0o600);

/// A job's log, and the directories it sits in under the session's working
/// tree.
///
/// Each is opened from the descriptor of the one above it and never through a
/// link, so a checkout that commits `.{application}` or its `jobs` directory as
/// a symlink cannot send a job's output anywhere else. Everything after that
/// goes through the descriptors: the child's writes, every read, the size
/// check and the removal. The path is kept only to be shown.
pub(super) struct Log {
    path: PathBuf,
    file: File,
    name: OsString,
    jobs: OwnedFd,
    application: OwnedFd,
    application_name: OsString,
    checkout: OwnedFd,
}

impl Log {
    /// Create the log for job `id`, readable and writable by this user alone,
    /// and the directories it needs.
    ///
    /// The log is always a new file, so nothing already at its name is
    /// opened, and a link at either directory level is refused.
    pub(super) fn create(checkout: &Path, application: &Application, id: &str) -> io::Result<Self> {
        let root = nix::fcntl::open(
            checkout,
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
            Mode::empty(),
        )?;
        let application_name = OsString::from(application.directory());
        let application_directory = descend(&root, &application_name)?;
        let jobs = descend(&application_directory, OsStr::new(JOB_LOG_DIRECTORY))?;
        let name = OsString::from(log_name(id));
        let file = openat(
            &jobs,
            name.as_os_str(),
            OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            LOG_MODE,
        )?;
        Ok(Self {
            path: log_path(checkout, application, id),
            file: File::from(file),
            name,
            jobs,
            application: application_directory,
            application_name,
            checkout: root,
        })
    }

    /// Where the log is, for the receipt.
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// Another descriptor for the log, for the child to write to.
    pub(super) fn writer(&self) -> io::Result<File> {
        self.file.try_clone()
    }

    /// How many bytes have been written so far.
    pub(super) fn size(&self) -> io::Result<u64> {
        self.file.metadata().map(|metadata| metadata.len())
    }

    /// Up to `limit` bytes from `offset`, fewer at the end of what has been
    /// written.
    ///
    /// Positioned reads, so the offset the child is writing at is never
    /// moved.
    pub(super) fn read(&self, offset: u64, limit: usize) -> io::Result<Vec<u8>> {
        let mut buffer = vec![0; limit];
        let mut filled = 0;
        while filled < limit {
            let Some(position) = offset.checked_add(filled as u64) else {
                break;
            };
            match self.file.read_at(&mut buffer[filled..], position) {
                Ok(0) => break,
                Ok(read) => filled += read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        buffer.truncate(filled);
        Ok(buffer)
    }

    /// Remove the log, then each directory it needed once it is left empty.
    ///
    /// Removing a directory that still holds anything fails, which is the
    /// whole of the emptiness check.
    pub(super) fn discard(&self) {
        let _ = unlinkat(
            &self.jobs,
            self.name.as_os_str(),
            UnlinkatFlags::NoRemoveDir,
        );
        if unlinkat(
            &self.application,
            JOB_LOG_DIRECTORY,
            UnlinkatFlags::RemoveDir,
        )
        .is_err()
        {
            return;
        }
        let _ = unlinkat(
            &self.checkout,
            self.application_name.as_os_str(),
            UnlinkatFlags::RemoveDir,
        );
    }
}

/// The directory `name` inside `parent`, made if it is missing, opened
/// without following a link.
fn descend(parent: &OwnedFd, name: &OsStr) -> io::Result<OwnedFd> {
    match mkdirat(parent, name, DIRECTORY_MODE) {
        Ok(()) | Err(Errno::EEXIST) => {}
        Err(errno) => return Err(errno.into()),
    }
    openat(
        parent,
        name,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(|errno| refused(parent, name, errno))
}

/// Why `name` could not be opened as a directory, naming a link as a link:
/// which errno `O_NOFOLLOW` reports for one differs between platforms.
fn refused(parent: &OwnedFd, name: &OsStr, errno: Errno) -> io::Error {
    let linked = fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW).is_ok_and(|status| {
        SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT == SFlag::S_IFLNK
    });
    match linked {
        true => io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{} is a symbolic link, and job logs are never written through one",
                name.display()
            ),
        ),
        false => errno.into(),
    }
}
