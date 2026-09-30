use std::fs;
use std::fs::File;
use std::path::Path;
use std::path::PathBuf;

use nix::errno::Errno;
use nix::fcntl::OFlag;
use nix::fcntl::open;
use nix::fcntl::openat;
use nix::sys::stat::Mode;
use nix::sys::stat::mkdirat;

use super::EXCLUDE_PATH;

/// The directory under a common directory that holds a linked worktree's
/// own git directory, one subdirectory per worktree.
const WORKTREES: &str = "worktrees";

/// The file in a linked worktree's git directory that names the `.git` file
/// pointing back at it.
const BACK_LINK: &str = "gitdir";

/// Where git keeps a checkout's repository, as `git rev-parse` resolved it
/// from the checkout.
///
/// Everything in a run's checkout is the run's to write, `.git` included, so
/// none of it is taken on trust. The exclude file is written only inside a
/// git directory the checkout owns, which is one of two layouts:
///
/// - the top level holds `.git` as a directory, not a link, and git resolves
///   both the git directory and the common directory to it: a clone, or a
///   repository made in place;
/// - the top level holds `.git` as a file, not a link, naming a linked
///   worktree's own git directory, which sits in the `worktrees` directory of
///   the common directory git resolves and whose `gitdir` file names this
///   `.git` file back: what `git worktree add` makes, a linked worktree of a
///   clone included.
///
/// A `.git` file naming anything else, a submodule's git directory or one
/// made by `--separate-git-dir` among them, is skipped along with every
/// layout a run could plant: a git directory whose `commondir` leads out of
/// the checkout, another repository's git directory, or a link to one.
pub(super) struct GitLayout {
    top_level: PathBuf,
    git_directory: PathBuf,
    common_directory: PathBuf,
}

impl GitLayout {
    /// What asks git for the three directories, one to a line, as absolute
    /// paths.
    pub(super) const QUERY: &[&str] = &[
        "rev-parse",
        "--path-format=absolute",
        "--show-toplevel",
        "--git-dir",
        "--git-common-dir",
    ];

    /// The layout [`QUERY`](Self::QUERY) printed, unless it printed anything
    /// but three lines.
    pub(super) fn parse(printed: &str) -> Option<Self> {
        let mut lines = printed.lines();
        let layout = Self {
            top_level: PathBuf::from(lines.next()?),
            git_directory: PathBuf::from(lines.next()?),
            common_directory: PathBuf::from(lines.next()?),
        };
        lines.next().is_none().then_some(layout)
    }

    /// The exclude file of the repository `checkout` owns, opened to be read
    /// and appended to, and created along with the `info` directory it sits
    /// in if either is missing. Neither is ever reached through a link.
    pub(super) fn exclude(&self, checkout: &Path) -> Result<File, String> {
        let repository = self.owned(checkout)?;
        open_exclude(&repository)
            .map_err(|error| format!("{}: {error}", repository.join(EXCLUDE_PATH).display()))
    }

    /// The canonical directory the checkout's exclude file belongs in.
    ///
    /// The checkout has to be the top level git found: a directory inside
    /// another repository is not a checkout, and that repository is not its
    /// to write.
    fn owned(&self, checkout: &Path) -> Result<PathBuf, String> {
        let top_level = canonical(&self.top_level)?;
        if canonical(checkout)? != top_level {
            return Err(
                "the checkout is not the top level of the repository git found".to_string(),
            );
        }
        let pointer = top_level.join(".git");
        let kind = fs::symlink_metadata(&pointer)
            .map_err(|error| format!("{}: {error}", pointer.display()))?
            .file_type();
        let git_directory = canonical(&self.git_directory)?;
        let common_directory = canonical(&self.common_directory)?;
        if kind.is_dir() {
            return (git_directory == pointer && common_directory == pointer)
                .then_some(pointer)
                .ok_or_else(|| "the checkout's .git is not the repository git found".to_string());
        }
        if !kind.is_file() {
            return Err("the checkout's .git is a link".to_string());
        }
        if git_directory.parent() != Some(common_directory.join(WORKTREES).as_path()) {
            return Err(
                "the checkout's .git names no linked worktree of its repository".to_string(),
            );
        }
        let named = fs::read_to_string(git_directory.join(BACK_LINK))
            .map_err(|error| format!("the linked worktree names no checkout: {error}"))?;
        let named = git_directory.join(named.trim_end_matches(['\n', '\r']));
        if canonical(&named)? != pointer {
            return Err("the linked worktree belongs to another checkout".to_string());
        }
        Ok(common_directory)
    }
}

/// The exclude file under `repository`, each step opened through the
/// descriptor of the directory before it and none through a link, so a run
/// that swaps `info` or `exclude` for a link between steps is refused rather
/// than followed.
fn open_exclude(repository: &Path) -> nix::Result<File> {
    let exclude = Path::new(EXCLUDE_PATH);
    let (Some(information), Some(name)) = (exclude.parent(), exclude.file_name()) else {
        return Err(Errno::EINVAL);
    };
    let directory = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let repository = open(repository, directory, Mode::empty())?;
    match mkdirat(&repository, information, Mode::from_bits_truncate(0o777)) {
        Ok(()) | Err(Errno::EEXIST) => {}
        Err(error) => return Err(error),
    }
    let information = openat(&repository, information, directory, Mode::empty())?;
    let file = openat(
        &information,
        name,
        OFlag::O_RDWR | OFlag::O_APPEND | OFlag::O_CREAT | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o666),
    )?;
    Ok(File::from(file))
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::GitLayout;
    use super::open_exclude;

    const FOREIGN: &str = "# someone else's\n";

    /// A repository directory and a directory outside it, side by side.
    fn planted() -> (TempDir, PathBuf, PathBuf) {
        let root = TempDir::new().expect("a temporary directory");
        let repository = root.path().join("repository");
        let outside = root.path().join("outside");
        for created in [&repository, &outside] {
            fs::create_dir(created).expect("the fixture's directories are created");
        }
        (root, repository, outside)
    }

    /// Checking `info` and then opening `exclude` by path left a window in
    /// which a run could swap `info` for a link and have the append follow
    /// it. The open itself now refuses a linked `info`.
    #[test]
    fn a_linked_info_directory_is_refused_by_the_open_itself() {
        let (_root, repository, outside) = planted();
        symlink(&outside, repository.join("info")).expect("the planted link");

        assert!(open_exclude(&repository).is_err());
        assert!(
            !outside.join("exclude").exists(),
            "the open followed a linked info directory"
        );
    }

    #[test]
    fn a_linked_exclude_file_is_refused_by_the_open_itself() {
        let (_root, repository, outside) = planted();
        let target = outside.join("exclude");
        fs::write(&target, FOREIGN).expect("the file outside");
        fs::create_dir(repository.join("info")).expect("the info directory");
        symlink(&target, repository.join("info").join("exclude")).expect("the planted link");

        assert!(open_exclude(&repository).is_err());
        assert_eq!(
            fs::read_to_string(&target).expect("the file outside reads"),
            FOREIGN,
            "the open followed a linked exclude file"
        );
    }

    #[test]
    fn a_missing_info_directory_and_exclude_file_are_created() {
        let (_root, repository, _outside) = planted();

        open_exclude(&repository).expect("the exclude file opens");

        assert!(repository.join("info").join("exclude").is_file());
    }

    /// A path with a line break in it prints as more than one line, and
    /// reading it as three would name directories git never printed.
    #[test]
    fn only_three_lines_read_as_a_layout() {
        assert!(GitLayout::parse("/top\n/top/.git\n/top/.git\n").is_some());
        assert!(GitLayout::parse("/top\n/top/.git\n").is_none());
        assert!(GitLayout::parse("/top\n/top\n/.git\n/top/.git\n").is_none());
    }
}
