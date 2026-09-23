use crate::checkout::Checkout;
use crate::git::GitError;
use crate::git::GitResult;
use crate::git::hardening::directories;
use crate::git::hardening::unborrowed;
use crate::git::hardening::unlinked;
use crate::git::native;
use std::ffi::OsStr;
use std::fs::Metadata;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::path::PathBuf;

/// What marks the top of a checkout: a clone's git directory, or a linked
/// worktree's file naming its own.
const GIT_MARKER: &str = ".git";

/// The directory of a repository's own records of its worktrees.
const WORKTREE_RECORDS: &str = "worktrees";

/// The file in a worktree's record naming its `.git` file.
const GITDIR: &str = "gitdir";

/// The file in a worktree's record naming the git directory it shares.
const COMMONDIR: &str = "commondir";

/// The longest path Linux resolves: a record holding more names nothing a
/// checkout's `.git` can be.
const LONGEST_PATH: u64 = 4096;

/// Where the git directories a command reads and writes must be for it to
/// reach no repository but the one it was pointed at. Git finds them from
/// what stands in the checkout -- a `.git` link, the file naming a linked
/// worktree's own directory, the record there naming the one it shares --
/// and a run can rewrite any of those to hand every later command another
/// clone's refs, index and configuration.
#[derive(Debug)]
pub(crate) enum Anchor {
    /// A checkout the caller named by its top and bound to its clone: a
    /// base clone, whose `.git` is its own git directory and the shared one
    /// at once, or a linked worktree, whose `.git` file names its own among
    /// the records of the clone's.
    Checkout(Checkout),
    /// A git directory a command is bound to by path, its own and the
    /// shared one at once.
    Bound(PathBuf),
}

impl Anchor {
    /// Refuse, before git is run to locate anything, a checkout whose top
    /// holds no `.git` with [`GitError::NotACheckoutTop`], and one whose
    /// `.git` is a link with [`GitError::LinkedPath`]: git run below the top
    /// of a checkout, or where there is none, reaches whichever repository
    /// encloses the path.
    pub(crate) fn marked(&self) -> GitResult<()> {
        match self {
            Self::Checkout(checkout) => marker(checkout.top()).map(drop),
            Self::Bound(_) => Ok(()),
        }
    }

    /// Refuse the git directories [`crate::git::LOCATING`] printed unless
    /// they are this anchor's own, hold no symbolic link, as [`unlinked`]
    /// looks for one, and borrow no objects, as [`unborrowed`] looks for a
    /// store they borrow from.
    pub(crate) fn admits(&self, located: &[u8]) -> GitResult<()> {
        self.holds(located)?;
        unlinked(located)?;
        unborrowed(located)
    }

    /// Refuse the git directories [`crate::git::LOCATING`] printed unless
    /// they are this anchor's own, looking at what stands at each without
    /// following a link. A checkout's top with no `.git` is refused with
    /// [`GitError::NotACheckoutTop`], a `.git` that is a link with
    /// [`GitError::LinkedPath`], and every other mismatch, including what
    /// cannot be looked at, with [`GitError::RedirectedGitDirectory`]: a
    /// shared git directory that is not the `.git` directory of the clone
    /// the checkout is bound to is one, however consistent the worktree
    /// record leading to it is. Every path is compared by its real path, so
    /// a checkout named through a linked directory above it is still its
    /// own.
    fn holds(&self, located: &[u8]) -> GitResult<()> {
        let (own, shared) = directories(located)?;
        let (Some(own), Some(shared)) = (real(&own), real(&shared)) else {
            return Err(GitError::RedirectedGitDirectory);
        };
        let confirmed = match self {
            Self::Checkout(checkout) => {
                let (marker, details) = marker(checkout.top())?;
                cloned(checkout.repository(), &shared)
                    && match details.is_dir() {
                        true => own == marker && shared == marker,
                        false => details.is_file() && linked(&marker, &own, &shared),
                    }
            }
            Self::Bound(directory) => {
                real(directory).is_some_and(|directory| own == directory && shared == directory)
            }
        };
        match confirmed {
            true => Ok(()),
            false => Err(GitError::RedirectedGitDirectory),
        }
    }

    /// The real path of the git directory of the base clone whose top is
    /// `top`, refusing a top with no `.git` with
    /// [`GitError::NotACheckoutTop`], a `.git` that is a link with
    /// [`GitError::LinkedPath`] and one that is not a directory with
    /// [`GitError::RedirectedGitDirectory`].
    pub(crate) fn own(top: &Path) -> GitResult<PathBuf> {
        let (marker, details) = marker(top)?;
        match details.is_dir() {
            true => Ok(marker),
            false => Err(GitError::RedirectedGitDirectory),
        }
    }
}

/// The real path of the `.git` standing at the checkout whose top is `top`,
/// and what stands there, looked at without following a link. A top that is
/// not there, or not a directory, and one with nothing at its `.git`, are
/// refused with [`GitError::NotACheckoutTop`]; a `.git` that is a link with
/// [`GitError::LinkedPath`]; and one that cannot be looked at with
/// [`GitError::RedirectedGitDirectory`].
fn marker(top: &Path) -> GitResult<(PathBuf, Metadata)> {
    let top = std::fs::canonicalize(top).map_err(|error| match absent(&error) {
        true => GitError::NotACheckoutTop,
        false => GitError::Io(error),
    })?;
    let marker = top.join(GIT_MARKER);
    let details = std::fs::symlink_metadata(&marker).map_err(|error| match absent(&error) {
        true => GitError::NotACheckoutTop,
        false => GitError::RedirectedGitDirectory,
    })?;
    match details.file_type().is_symlink() {
        true => Err(GitError::LinkedPath),
        false => Ok((marker, details)),
    }
}

/// Whether a look at a path failed because nothing stands there, or because
/// something on the way to it is not a directory.
fn absent(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

/// Whether `shared` is the `.git` directory, and not a link to one, of the
/// clone whose top is `repository`.
fn cloned(repository: &Path, shared: &Path) -> bool {
    let directory = repository.join(GIT_MARKER);
    std::fs::symlink_metadata(&directory).is_ok_and(|details| details.is_dir())
        && real(&directory).as_deref() == Some(shared)
}

/// Whether `own` and `shared` are the git directories of the linked
/// worktree whose `.git` file stands at `marker`: `own` is a record among
/// `shared`'s worktrees, and the record names `marker` as its `.git` and
/// `shared` as the directory it shares, as git reads both, relative to the
/// record.
fn linked(marker: &Path, own: &Path, shared: &Path) -> bool {
    let recorded = own
        .parent()
        .filter(|records| records.file_name() == Some(OsStr::new(WORKTREE_RECORDS)))
        .and_then(Path::parent);
    recorded == Some(shared)
        && pointed(own, GITDIR).as_deref() == Some(marker)
        && pointed(own, COMMONDIR).as_deref() == Some(shared)
}

/// The real path of what the file `name` in the record `own` names, read
/// as git reads it: the line end dropped and a relative path taken from the
/// record.
fn pointed(own: &Path, name: &str) -> Option<PathBuf> {
    let content = pointer(&own.join(name))?;
    real(&own.join(native(content.trim_ascii_end())?))
}

/// What the regular file at `file` holds, when it holds no more than
/// [`LONGEST_PATH`] bytes. The file is opened without following a link or
/// waiting on a pipe that stands in its place.
fn pointer(file: &Path) -> Option<Vec<u8>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags((nix::fcntl::OFlag::O_NOFOLLOW | nix::fcntl::OFlag::O_NONBLOCK).bits());
    let handle = options.open(file).ok()?;
    if !handle.metadata().ok()?.is_file() {
        return None;
    }
    let mut content = Vec::new();
    handle
        .take(LONGEST_PATH + 1)
        .read_to_end(&mut content)
        .ok()?;
    (content.len() as u64 <= LONGEST_PATH).then_some(content)
}

/// `path` with every link on the way resolved, when it is there.
fn real(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}
