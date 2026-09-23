use std::path::PathBuf;

/// How `git worktree list --porcelain` introduces each field it reports.
const WORKTREE: &str = "worktree ";
const BRANCH: &str = "branch ";
const LOCKED: &str = "locked";

/// One worktree a repository has registered, as `git worktree list
/// --porcelain -z` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeEntry {
    pub(crate) path: PathBuf,
    pub(crate) branch: Option<String>,
    pub(crate) locked: bool,
}

impl WorktreeEntry {
    /// Every worktree in a listing, the repository's own first. Records are
    /// separated by an empty field.
    pub(crate) fn parse(listing: &[u8]) -> Vec<Self> {
        let listing = String::from_utf8_lossy(listing);
        let mut entries: Vec<Self> = Vec::new();
        for field in listing.split('\0') {
            if let Some(path) = field.strip_prefix(WORKTREE) {
                entries.push(Self {
                    path: PathBuf::from(path),
                    branch: None,
                    locked: false,
                });
            } else if let Some(entry) = entries.last_mut() {
                if let Some(branch) = field.strip_prefix(BRANCH) {
                    entry.branch = Some(branch.to_string());
                } else if field == LOCKED || field.starts_with(&format!("{LOCKED} ")) {
                    entry.locked = true;
                }
            }
        }
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_worktree_is_read_with_its_branch_and_its_lock() {
        let listing = "worktree /work/base\0HEAD 1111\0branch refs/heads/main\0\0\
                       worktree /work/area-worktrees/one\0HEAD 2222\0detached\0\0\
                       worktree /work/area-worktrees/two words\0HEAD 3333\0detached\0locked busy\0\0\
                       worktree /work/area-worktrees/three\0HEAD 4444\0branch refs/heads/task\0locked\0\0";

        assert_eq!(
            WorktreeEntry::parse(listing.as_bytes()),
            vec![
                WorktreeEntry {
                    path: PathBuf::from("/work/base"),
                    branch: Some("refs/heads/main".to_string()),
                    locked: false,
                },
                WorktreeEntry {
                    path: PathBuf::from("/work/area-worktrees/one"),
                    branch: None,
                    locked: false,
                },
                WorktreeEntry {
                    path: PathBuf::from("/work/area-worktrees/two words"),
                    branch: None,
                    locked: true,
                },
                WorktreeEntry {
                    path: PathBuf::from("/work/area-worktrees/three"),
                    branch: Some("refs/heads/task".to_string()),
                    locked: true,
                },
            ]
        );
    }
}
