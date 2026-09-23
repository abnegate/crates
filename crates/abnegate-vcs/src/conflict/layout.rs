use std::path::Path;
use std::path::PathBuf;

/// The subdirectory of the throwaway root holding the reproduced merge.
const CHECKOUT_DIRECTORY: &str = "checkout";

/// The subdirectory holding the repository, outside the checkout a repair edits.
const GIT_DIRECTORY: &str = "git";

/// The subdirectory a repair is given as its home, cache and temporary space.
const ISOLATION_DIRECTORY: &str = "isolation";

/// The empty directory git runs with as its home, outside everything a repair
/// can write, so no ignore or attributes file there can reach git.
const HOME_DIRECTORY: &str = "home";

/// Where each part of a reproduced conflict lives under its throwaway root.
#[derive(Debug, Clone)]
pub(super) struct Layout {
    pub(super) checkout: PathBuf,
    pub(super) git: PathBuf,
    pub(super) isolation: PathBuf,
    pub(super) home: PathBuf,
}

impl Layout {
    pub(super) fn under(root: &Path) -> Self {
        Self {
            checkout: root.join(CHECKOUT_DIRECTORY),
            git: root.join(GIT_DIRECTORY),
            isolation: root.join(ISOLATION_DIRECTORY),
            home: root.join(HOME_DIRECTORY),
        }
    }

    /// Make the directories git does not make itself.
    pub(super) fn create(&self) -> std::io::Result<()> {
        for directory in [&self.checkout, &self.isolation, &self.home] {
            std::fs::create_dir(directory)?;
        }
        Ok(())
    }
}
