use std::fs::DirBuilder;
use std::io;
use std::io::Write;
use std::path::Path;

use tempfile::NamedTempFile;

use crate::error::ConfigError;

#[cfg(unix)]
const DIRECTORY_MODE: u32 = 0o700;

/// A file that holds credentials, so it is only ever readable by its owner.
///
/// The contents are written to a temporary file in the destination directory,
/// which is created owner-only before a byte is written, and then renamed over
/// the destination. A reader never sees a partial file, and a symlink at the
/// destination is replaced rather than followed.
pub(crate) struct PrivateFile<'path> {
    path: &'path Path,
}

impl<'path> PrivateFile<'path> {
    pub(crate) fn new(path: &'path Path) -> Self {
        Self { path }
    }

    pub(crate) fn write(&self, contents: &[u8]) -> Result<(), ConfigError> {
        self.replace(contents).map_err(|source| ConfigError::Write {
            path: self.path.to_path_buf(),
            source,
        })
    }

    fn replace(&self, contents: &[u8]) -> io::Result<()> {
        let directory = self.directory();
        create_directory(directory)?;

        let mut temporary = NamedTempFile::new_in(directory)?;
        temporary.write_all(contents)?;
        temporary.as_file().sync_all()?;
        temporary.persist(self.path)?;

        Ok(())
    }

    fn directory(&self) -> &Path {
        match self.path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        }
    }
}

fn create_directory(directory: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        builder.mode(DIRECTORY_MODE);
    }

    builder.create(directory)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn the_contents_are_written() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("secret.txt");

        PrivateFile::new(&path).write(b"contents").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "contents");
    }

    #[test]
    fn an_existing_file_is_replaced() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("secret.txt");
        fs::write(&path, "a much longer previous body").unwrap();

        PrivateFile::new(&path).write(b"short").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "short");
    }

    #[test]
    fn no_temporary_file_is_left_behind() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("secret.txt");

        PrivateFile::new(&path).write(b"contents").unwrap();

        let entries: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, ["secret.txt"]);
    }

    #[test]
    fn a_failure_names_the_destination() {
        let directory = TempDir::new().unwrap();
        let blocker = directory.path().join("blocker");
        fs::write(&blocker, "").unwrap();
        let path = blocker.join("secret.txt");

        let error = PrivateFile::new(&path).write(b"contents").unwrap_err();

        assert!(
            matches!(&error, ConfigError::Write { path: reported, .. } if reported == &path),
            "{error:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_file_and_its_new_directories_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TempDir::new().unwrap();
        let nested = directory.path().join("one").join("two");
        let path = nested.join("secret.txt");

        PrivateFile::new(&path).write(b"contents").unwrap();

        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&nested), DIRECTORY_MODE);
        assert_eq!(mode(&directory.path().join("one")), DIRECTORY_MODE);
    }

    #[cfg(unix)]
    #[test]
    fn a_world_readable_file_becomes_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TempDir::new().unwrap();
        let path = directory.path().join("secret.txt");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        PrivateFile::new(&path).write(b"new").unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_replaced_rather_than_followed() {
        let directory = TempDir::new().unwrap();
        let target = directory.path().join("target.txt");
        let path = directory.path().join("secret.txt");
        fs::write(&target, "untouched").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();

        PrivateFile::new(&path).write(b"contents").unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
        assert!(
            !fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "contents");
    }
}
