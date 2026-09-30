use std::path::Path;
use std::path::PathBuf;

use super::denied::Denied;
use super::identity::Identity;
use crate::tool::ToolContext;

const PROC: &str = "/proc";

/// The links `/proc` keeps to the reader's own entry.
const OWN_ENTRIES: &[&str] = &["self", "thread-self"];

/// Where a process reads its own descriptors as files: a link into `/proc` on
/// Linux, and a directory of its own on macOS.
const DESCRIPTORS: &str = "/dev/fd";

/// macOS's volume file system, which opens `/.vol/<device>/<inode>` by
/// identity, so the path names neither the file nor any directory above it.
const VOLUMES: &str = "/.vol";

/// What no file tool reaches however unrestricted its context: every path the
/// context denies, the reader's own descriptors, macOS's `/.vol`, and every
/// process's `/proc` entry.
///
/// Each is compared by what it is on disk rather than by how it is spelled: a
/// firmlink, a bind mount, or a name that differs only in case or in Unicode
/// normalization reaches it under a string that no comparison matches.
#[derive(Debug)]
pub(super) struct Withheld {
    denied: Vec<Denied>,
}

impl Withheld {
    /// What `context` withholds, its relative denied paths taken from its
    /// working directory.
    pub(super) fn of(context: &ToolContext) -> Self {
        let denied = context
            .denied
            .iter()
            .map(|path| context.working_directory.join(path))
            .chain([PathBuf::from(DESCRIPTORS), PathBuf::from(VOLUMES)])
            .filter_map(|path| Denied::of(&path))
            .collect();
        Self { denied }
    }

    /// Whether `resolved` is in a process's `/proc` entry, or at or under a
    /// withheld path: whether any ancestor of it that exists is a withheld
    /// directory, or the one a withheld path yet to be made will be made in.
    pub(super) fn holds(&self, resolved: &Path) -> bool {
        if per_process(resolved) {
            return true;
        }
        resolved.ancestors().any(|ancestor| {
            Identity::of(ancestor).is_some_and(|identity| {
                let below = resolved.strip_prefix(ancestor).unwrap_or(resolved);
                self.denied
                    .iter()
                    .any(|denied| denied.covers(identity, below))
            })
        })
    }

    /// Whether the directory at `path`, already known to be `identity`, is
    /// withheld whole, for a walk deciding whether to enter it.
    pub(super) fn holds_directory(&self, path: &Path, identity: Identity) -> bool {
        per_process(path) || self.denied.iter().any(|denied| denied.is(identity))
    }
}

/// Whether `path` is in one process's `/proc` entry.
///
/// Its `environ` holds whatever that process was started with, and the links
/// under it are magic: they reach a file by identity rather than by the name
/// they print, so where one leads cannot be judged by resolving it.
pub(super) fn per_process(path: &Path) -> bool {
    let Ok(rest) = path.strip_prefix(PROC) else {
        return false;
    };
    rest.components().next().is_some_and(|entry| {
        let name = entry.as_os_str();
        OWN_ENTRIES.iter().any(|own| name == *own)
            || name.as_encoded_bytes().iter().all(u8::is_ascii_digit)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_processs_own_proc_entry_is_withheld() {
        for path in [
            "/proc/1/environ",
            "/proc/48213",
            "/proc/48213/task/48214/environ",
            "/proc/48213/cwd/../auth.json",
            "/proc/self/environ",
            "/proc/thread-self/environ",
        ] {
            assert!(per_process(Path::new(path)), "{path}");
        }
        for path in [
            "/proc",
            "/proc/cpuinfo",
            "/proc/sys/kernel/hostname",
            "/procfs/1/environ",
            "/srv/proc/1/environ",
        ] {
            assert!(!per_process(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn the_default_context_withholds_only_what_every_context_does() {
        let context = ToolContext::default();
        let withheld = Withheld::of(&context);

        assert!(withheld.holds(Path::new("/proc/self/environ")));
        assert!(withheld.holds(Path::new("/.vol/1/2")));
        assert!(!withheld.holds(&context.working_directory));
    }
}
