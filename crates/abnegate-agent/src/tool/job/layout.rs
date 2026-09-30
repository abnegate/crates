use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::path::PathBuf;

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
        let exclude = repository.join(EXCLUDE_PATH);
        let information = exclude
            .parent()
            .ok_or("the exclude path names no directory")?;
        match fs::symlink_metadata(information) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(format!("{} is not a directory", information.display())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(information).map_err(|error| error.to_string())?;
            }
            Err(error) => return Err(error.to_string()),
        }
        OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(&exclude)
            .map_err(|error| format!("{}: {error}", exclude.display()))
    }

    /// The canonical directory the checkout's exclude file belongs in.
    fn owned(&self, checkout: &Path) -> Result<PathBuf, String> {
        let top_level = canonical(&self.top_level)?;
        if !canonical(checkout)?.starts_with(&top_level) {
            return Err("git found a repository outside the checkout".to_string());
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
        if canonical(Path::new(named.trim_end_matches(['\n', '\r'])))? != pointer {
            return Err("the linked worktree belongs to another checkout".to_string());
        }
        Ok(common_directory)
    }
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::GitLayout;

    /// A path with a line break in it prints as more than one line, and
    /// reading it as three would name directories git never printed.
    #[test]
    fn only_three_lines_read_as_a_layout() {
        assert!(GitLayout::parse("/top\n/top/.git\n/top/.git\n").is_some());
        assert!(GitLayout::parse("/top\n/top/.git\n").is_none());
        assert!(GitLayout::parse("/top\n/top\n/.git\n/top/.git\n").is_none());
    }
}
