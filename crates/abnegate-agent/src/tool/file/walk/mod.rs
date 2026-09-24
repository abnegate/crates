mod visit;

use std::collections::HashSet;
use std::fs;
use std::fs::DirEntry;
use std::fs::FileType;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

pub(super) use visit::Visit;

use crate::tool::ToolError;

/// Deepest a walk descends below the directory it started from.
pub(super) const MAXIMUM_WALK_DEPTH: usize = 64;

/// Most entries one walk looks at before it gives up.
pub(super) const MAXIMUM_WALK_ENTRIES: usize = 100_000;

/// Longest one walk runs, kept inside the default tool timeout so a walk
/// that runs out of time still reports what it found.
pub(super) const WALK_TIME_LIMIT: Duration = Duration::from_secs(20);

/// A depth-first walk of a directory tree that never follows a link.
///
/// A symlinked directory is shown to the visitor as the link it is and never
/// entered, and a directory reached twice by any route (a bind mount, a
/// hard-linked directory) is entered once, so a tree that loops back on
/// itself still ends. Depth, entry and time budgets bound whatever is left.
pub(super) struct Walk {
    deadline: Instant,
    remaining: usize,
    visited: HashSet<(u64, u64)>,
    stopped: Option<&'static str>,
}

impl Walk {
    pub(super) fn new(limit: Duration) -> Self {
        Self {
            deadline: Instant::now() + limit,
            remaining: MAXIMUM_WALK_ENTRIES,
            visited: HashSet::new(),
            stopped: None,
        }
    }

    /// Why the walk ended before it had seen the whole tree, if it did.
    pub(super) fn stopped(&self) -> Option<&'static str> {
        self.stopped
    }

    /// Show `visit` every entry beneath `root`, entering the directories it
    /// asks to.
    ///
    /// Only a `root` that cannot be read is an error: a directory further down
    /// that cannot be read is left out, as the entries it held would be.
    pub(super) fn run(
        &mut self,
        root: &Path,
        mut visit: impl FnMut(&DirEntry, FileType) -> Visit,
    ) -> Result<(), ToolError> {
        let entries = fs::read_dir(root)
            .map_err(|error| ToolError::Execution(format!("Cannot read directory: {error}")))?;
        if let Ok(metadata) = fs::metadata(root) {
            self.visited.insert((metadata.dev(), metadata.ino()));
        }

        let mut pending: Vec<(fs::ReadDir, usize)> = vec![(entries, 0)];
        while let Some((entries, depth)) = pending.last_mut() {
            let depth = *depth;
            let Some(entry) = entries.next() else {
                pending.pop();
                continue;
            };
            if let Some(reason) = self.exhausted() {
                self.stopped = Some(reason);
                return Ok(());
            }
            self.remaining -= 1;

            let Ok(entry) = entry else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            match visit(&entry, file_type) {
                Visit::Stop => return Ok(()),
                Visit::Skip => {}
                Visit::Descend if file_type.is_dir() => {
                    if let Some(child) = self.enter(entry.path(), depth) {
                        pending.push((child, depth + 1));
                    }
                }
                Visit::Descend => {}
            }
        }
        Ok(())
    }

    fn exhausted(&self) -> Option<&'static str> {
        if self.remaining == 0 {
            return Some("too many entries");
        }
        if Instant::now() >= self.deadline {
            return Some("out of time");
        }
        None
    }

    fn enter(&mut self, directory: PathBuf, depth: usize) -> Option<fs::ReadDir> {
        if depth + 1 > MAXIMUM_WALK_DEPTH {
            self.stopped = Some("too deep");
            return None;
        }
        let metadata = fs::symlink_metadata(&directory).ok()?;
        if !metadata.is_dir() || !self.visited.insert((metadata.dev(), metadata.ino())) {
            return None;
        }
        fs::read_dir(directory).ok()
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    fn names(root: &Path) -> (Vec<String>, Option<&'static str>) {
        let mut walk = Walk::new(WALK_TIME_LIMIT);
        let mut seen = Vec::new();
        walk.run(root, |entry, _| {
            seen.push(
                entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .display()
                    .to_string(),
            );
            Visit::Descend
        })
        .unwrap();
        seen.sort();
        (seen, walk.stopped())
    }

    #[test]
    fn a_link_to_a_directory_is_seen_and_never_entered() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("real")).unwrap();
        std::fs::write(root.path().join("real/file"), "x").unwrap();
        symlink(root.path().join("real"), root.path().join("alias")).unwrap();

        let (seen, stopped) = names(root.path());

        assert_eq!(seen, ["alias", "real", "real/file"]);
        assert_eq!(stopped, None);
    }

    #[test]
    fn a_walk_past_its_depth_says_so() {
        let root = tempfile::tempdir().unwrap();
        let mut deep = root.path().to_path_buf();
        for _ in 0..=MAXIMUM_WALK_DEPTH {
            deep.push("d");
        }
        std::fs::create_dir_all(&deep).unwrap();

        let (seen, stopped) = names(root.path());

        assert_eq!(seen.len(), MAXIMUM_WALK_DEPTH + 1);
        assert_eq!(stopped, Some("too deep"));
    }

    #[test]
    fn a_walk_out_of_time_says_so() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), "x").unwrap();
        let mut walk = Walk::new(Duration::ZERO);

        walk.run(root.path(), |_, _| Visit::Descend).unwrap();

        assert_eq!(walk.stopped(), Some("out of time"));
    }
}
