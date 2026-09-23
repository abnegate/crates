//! Opens confined beneath the working directory.
//!
//! Checking a path and then opening it by name resolves the same name twice,
//! and a process sharing the working directory can swap a checked component for
//! a symlink between the two. Every open here resolves once, against a
//! descriptor for the root, so the path that was checked is the path that is
//! opened.

mod access;
mod name;
mod target;

pub(crate) use access::Access;

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, openat, readlinkat};
use nix::sys::stat::{Mode, SFlag, fstatat, mkdirat};
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Component, Path};

use super::{ToolContext, ToolError};
use name::Name;
use target::Target;

/// `openat2` answers `EXDEV` when `RESOLVE_BENEATH` would be broken, so the
/// walk answers the same and one mapping covers both resolutions.
const ESCAPED: Errno = Errno::EXDEV;
const FILE_MODE: Mode = Mode::from_bits_truncate(0o666);
const DIRECTORY_MODE: Mode = Mode::from_bits_truncate(0o777);
const LINKS: usize = 40;

pub(crate) fn open(context: &ToolContext, path: &Path, access: Access) -> Result<File, ToolError> {
    if context.unrestricted {
        return access
            .options()
            .open(context.cwd.join(path))
            .map_err(|error| failed("open file", error));
    }

    resolve(
        &context.cwd,
        under(&context.cwd, path),
        Target::File(access),
        false,
    )
    .map(File::from)
    .map_err(reported)
    .map_err(|error| failed("open file", error))
}

pub(crate) fn create_dir_all(context: &ToolContext, path: &Path) -> Result<(), ToolError> {
    if context.unrestricted {
        return fs::create_dir_all(context.cwd.join(path))
            .map_err(|error| failed("create directory", error));
    }

    resolve(
        &context.cwd,
        under(&context.cwd, path),
        Target::Directory,
        true,
    )
    .map(drop)
    .map_err(reported)
    .map_err(|error| failed("create directory", error))
}

fn failed(what: &str, error: io::Error) -> ToolError {
    if error.raw_os_error() == Some(ESCAPED as i32) {
        return ToolError::Execution("Path escapes working directory".to_string());
    }
    ToolError::Execution(format!("Cannot {what}: {error}"))
}

fn reported(errno: Errno) -> io::Error {
    io::Error::from_raw_os_error(errno as i32)
}

/// The part of `path` that names something under `root`.
///
/// A path the caller already joined to the root strips back to the names below
/// it; anything else is passed through and refused by the walk as an escape.
fn under<'a>(root: &Path, path: &'a Path) -> &'a Path {
    path.strip_prefix(root).unwrap_or(path)
}

fn resolve(root: &Path, path: &Path, target: Target, create: bool) -> Result<OwnedFd, Errno> {
    if let Target::File(access) = target
        && let Some(opened) = kernel_resolved(root, path, access)
    {
        return opened;
    }
    walk(root, path, target, create)
}

/// `openat2` resolves the whole path in the kernel, so no component is opened
/// by a name that another process could still change.
#[cfg(target_os = "linux")]
fn kernel_resolved(root: &Path, path: &Path, access: Access) -> Option<Result<OwnedFd, Errno>> {
    use nix::fcntl::{OpenHow, ResolveFlag, openat2};

    let root = match directory(root) {
        Ok(root) => root,
        Err(errno) => return Some(Err(errno)),
    };
    let how = OpenHow::new()
        .flags(access.flags() | OFlag::O_CLOEXEC)
        .mode(FILE_MODE)
        .resolve(ResolveFlag::RESOLVE_BENEATH | ResolveFlag::RESOLVE_NO_MAGICLINKS);

    match openat2(&root, path, how) {
        // Kernels before 5.6, and the seccomp filters a sandbox installs, leave
        // the per-component walk as the only confined resolution.
        Err(Errno::ENOSYS | Errno::EPERM | Errno::EINVAL) => None,
        opened => Some(opened),
    }
}

#[cfg(not(target_os = "linux"))]
fn kernel_resolved(_root: &Path, _path: &Path, _access: Access) -> Option<Result<OwnedFd, Errno>> {
    None
}

/// Open each component from the descriptor of the one above it.
///
/// `O_NOFOLLOW` means no component is ever followed by the kernel: a link is
/// read here instead, and its own names are walked the same way, so resolution
/// cannot leave the descriptor it started from. `..` unwinds the descriptors
/// already held and is refused once there are none, which is what
/// `RESOLVE_BENEATH` does on the kernel path.
fn walk(root: &Path, path: &Path, target: Target, create: bool) -> Result<OwnedFd, Errno> {
    let root = directory(root)?;
    let mut held: Vec<OwnedFd> = Vec::new();
    let mut pending = names(path)?;
    let mut links = LINKS;

    while let Some(name) = pending.pop_front() {
        let name = match name {
            Name::Parent => {
                if held.pop().is_none() {
                    return Err(ESCAPED);
                }
                continue;
            }
            Name::Entry(name) => name,
        };

        let directory = held.last().unwrap_or(&root);
        let last = pending.is_empty();
        let opened = match target {
            Target::File(access) if last => openat(
                directory,
                name.as_os_str(),
                access.flags() | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
                FILE_MODE,
            ),
            _ => descend(directory, name.as_os_str(), create),
        };

        match opened {
            Ok(opened) if last => return Ok(opened),
            Ok(opened) => held.push(opened),
            Err(error) => {
                let Some(link) = linked(directory, name.as_os_str(), error)? else {
                    return Err(error);
                };
                links = links.checked_sub(1).ok_or(Errno::ELOOP)?;
                for name in names(Path::new(&link))?.into_iter().rev() {
                    pending.push_front(name);
                }
            }
        }
    }

    match target {
        Target::Directory => Ok(held.pop().unwrap_or(root)),
        Target::File(_) => Err(Errno::EISDIR),
    }
}

fn names(path: &Path) -> Result<VecDeque<Name>, Errno> {
    let mut names = VecDeque::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => names.push_back(Name::Entry(name.to_os_string())),
            Component::ParentDir => names.push_back(Name::Parent),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => return Err(ESCAPED),
        }
    }
    Ok(names)
}

fn directory(path: &Path) -> Result<OwnedFd, Errno> {
    nix::fcntl::open(
        path,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
}

fn descend(directory: &OwnedFd, name: &OsStr, create: bool) -> Result<OwnedFd, Errno> {
    if create {
        match mkdirat(directory, name, DIRECTORY_MODE) {
            Ok(()) | Err(Errno::EEXIST) => {}
            Err(error) => return Err(error),
        }
    }
    openat(
        directory,
        name,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
}

/// The link a refused open tripped over, if it was a link at all.
///
/// Which errno `O_NOFOLLOW` reports for a link differs between the platforms
/// and with the other flags in the open, so the entry is stated rather than the
/// errno read.
fn linked(directory: &OwnedFd, name: &OsStr, error: Errno) -> Result<Option<OsString>, Errno> {
    if error == Errno::ENOENT {
        return Ok(None);
    }
    let Ok(status) = fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW) else {
        return Ok(None);
    };
    if SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT != SFlag::S_IFLNK {
        return Ok(None);
    }

    let target = readlinkat(directory, name)?;
    if Path::new(&target).is_absolute() {
        // `RESOLVE_BENEATH` refuses an absolute link target outright; matching
        // it keeps the two resolutions answering the same on both platforms.
        return Err(ESCAPED);
    }
    Ok(Some(target))
}
