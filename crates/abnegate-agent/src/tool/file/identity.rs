use std::fs;
use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// A file as the filesystem identifies it, whatever name reached it.
///
/// A firmlink, a bind mount, and a name that differs only in case or in
/// Unicode normalization on a filesystem that folds them all reach one file
/// under a string that matches no other spelling of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Identity {
    device: u64,
    inode: u64,
}

impl Identity {
    /// Following links, as an open would.
    pub(super) fn of(path: &Path) -> Option<Self> {
        fs::metadata(path)
            .ok()
            .map(|metadata| Self::from(&metadata))
    }
}

impl From<&Metadata> for Identity {
    fn from(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn a_link_and_its_target_are_one_file_and_a_sibling_is_another() {
        let directory = tempdir().unwrap();
        let target = directory.path().join("target");
        let sibling = directory.path().join("sibling");
        fs::create_dir(&target).unwrap();
        fs::create_dir(&sibling).unwrap();
        std::os::unix::fs::symlink(&target, directory.path().join("link")).unwrap();

        let identity = Identity::of(&target).expect("a directory that exists");

        assert_eq!(Identity::of(&directory.path().join("link")), Some(identity));
        assert_ne!(Identity::of(&sibling), Some(identity));
        assert_eq!(Identity::of(&directory.path().join("missing")), None);
    }
}
