use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::path::PathBuf;

use zeroize::Zeroize;
use zeroize::Zeroizing;

use crate::error::SecretError;
use crate::random;
use crate::redact::REDACTED;

pub(crate) const KEY_BYTES: usize = 32;

const KEY_FILE_NAME: &str = "master.key";
const KEY_VARIABLE_SUFFIX: &str = "_MASTER_KEY";
const KEY_FILE_VARIABLE_SUFFIX: &str = "_MASTER_KEY_FILE";
const TEMPORARY_EXTENSION: &str = "tmp";
const CURRENT_DIRECTORY: &str = ".";
const TEMPORARY_NAME_BYTES: usize = 8;
#[cfg(unix)]
const KEY_FILE_MODE: u32 = 0o600;
#[cfg(unix)]
const KEY_DIRECTORY_MODE: u32 = 0o700;

/// The AES-256 key every envelope in a deployment is sealed with.
pub struct MasterKey {
    key: [u8; KEY_BYTES],
}

impl MasterKey {
    pub fn new(key: [u8; KEY_BYTES]) -> Self {
        Self { key }
    }

    /// A fresh key drawn from the operating system's random source.
    pub fn generate() -> Result<Self, SecretError> {
        let mut generated = Self {
            key: [0u8; KEY_BYTES],
        };
        random::fill(&mut generated.key)?;
        Ok(generated)
    }

    pub fn from_hex(hexadecimal: &str) -> Result<Self, SecretError> {
        let decoded =
            hex::decode(hexadecimal.trim()).map_err(|_| SecretError::InvalidHexadecimal)?;
        let decoded = Zeroizing::new(decoded);
        let key = decoded
            .as_slice()
            .try_into()
            .map_err(|_| SecretError::KeyLength {
                expected: KEY_BYTES,
                actual: decoded.len(),
            })?;
        Ok(Self { key })
    }

    /// The key as hexadecimal, zeroized when dropped.
    pub fn to_hex(&self) -> Zeroizing<String> {
        Zeroizing::new(hex::encode(self.key))
    }

    pub(crate) fn as_bytes(&self) -> &[u8; KEY_BYTES] {
        &self.key
    }
}

impl Drop for MasterKey {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

impl fmt::Debug for MasterKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "MasterKey({REDACTED})")
    }
}

/// Where `application` keeps its master key when no other source names one:
/// `~/.<application>/master.key`.
///
/// Fails rather than falling back to the working directory when there is no
/// home directory, so a key is never picked up from wherever the process
/// happens to start.
pub fn default_key_path(application: &str) -> Result<PathBuf, SecretError> {
    dirs::home_dir()
        .map(|home| key_path_under(&home, application))
        .ok_or(SecretError::NoHomeDirectory)
}

/// The master key `application` is configured with, in order of precedence:
/// the hexadecimal `<APPLICATION>_MASTER_KEY` variable, the path in
/// `<APPLICATION>_MASTER_KEY_FILE`, then [`default_key_path`].
///
/// `None` means no key is configured, which is the caller's cue to run in
/// plaintext mode. A key variable that is not UTF-8, or a missing home
/// directory when neither variable is set, is an error rather than a silent
/// fall through to plaintext mode.
pub fn load_master_key(application: &str) -> Result<Option<MasterKey>, SecretError> {
    load_master_key_from(application, |name| env::var_os(name), dirs::home_dir())
}

fn load_master_key_from(
    application: &str,
    variable: impl Fn(&str) -> Option<OsString>,
    home: Option<PathBuf>,
) -> Result<Option<MasterKey>, SecretError> {
    let prefix = environment_prefix(application);

    let key_variable = format!("{prefix}{KEY_VARIABLE_SUFFIX}");
    if let Some(value) = variable(&key_variable).filter(|value| !value.is_empty()) {
        let hexadecimal = value
            .into_string()
            .map_err(|_| SecretError::InvalidEnvironment {
                variable: key_variable,
            })?;
        return MasterKey::from_hex(&Zeroizing::new(hexadecimal)).map(Some);
    }

    let file_variable = format!("{prefix}{KEY_FILE_VARIABLE_SUFFIX}");
    if let Some(path) = variable(&file_variable).filter(|path| !path.is_empty()) {
        return read_key_file(Path::new(&path)).map(Some);
    }

    let home = home.ok_or(SecretError::NoHomeDirectory)?;
    let path = key_path_under(&home, application);
    if path.exists() {
        return read_key_file(&path).map(Some);
    }

    Ok(None)
}

/// Read a hexadecimal master key from `path`.
pub fn read_key_file(path: &Path) -> Result<MasterKey, SecretError> {
    let content = fs::read_to_string(path).map_err(|source| SecretError::ReadKeyFile {
        path: path.to_path_buf(),
        source,
    })?;
    MasterKey::from_hex(&Zeroizing::new(content))
}

/// Write `key` to `path` as hexadecimal, readable only by its owner.
///
/// The key goes into a new owner-only file beside `path`, which is then
/// renamed over it. The key is never readable by anyone else, even briefly; a
/// symbolic link at `path` is replaced rather than followed; and a file
/// already at `path` never receives the key, so its permissions and any hard
/// links to it do not carry over. Missing parent directories are created
/// owner-only, and the key and its directory entry are flushed to disk before
/// this returns.
pub fn write_key_file(path: &Path, key: &MasterKey) -> Result<(), SecretError> {
    let failed = |source: io::Error| SecretError::WriteKeyFile {
        path: path.to_path_buf(),
        source,
    };

    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(directory) = directory {
        create_private_directory(directory).map_err(failed)?;
    }

    let temporary = temporary_path(path)?;
    let file = create_private_file(&temporary).map_err(failed)?;
    let persisted =
        persist(file, key.to_hex().as_bytes()).and_then(|()| fs::rename(&temporary, path));
    if let Err(source) = persisted {
        let _ = fs::remove_file(&temporary);
        return Err(failed(source));
    }

    sync_directory(directory.unwrap_or(Path::new(CURRENT_DIRECTORY))).map_err(failed)
}

fn key_path_under(home: &Path, application: &str) -> PathBuf {
    home.join(format!(".{application}")).join(KEY_FILE_NAME)
}

fn temporary_path(path: &Path) -> Result<PathBuf, SecretError> {
    let name = path.file_name().ok_or_else(|| SecretError::WriteKeyFile {
        path: path.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidInput, "the path names no file"),
    })?;

    let mut suffix = [0u8; TEMPORARY_NAME_BYTES];
    random::fill(&mut suffix)?;

    let mut temporary = OsString::from(".");
    temporary.push(name);
    temporary.push(format!(".{}.{TEMPORARY_EXTENSION}", hex::encode(suffix)));
    Ok(path.with_file_name(temporary))
}

#[cfg(unix)]
fn create_private_directory(directory: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(KEY_DIRECTORY_MODE)
        .create(directory)
}

#[cfg(not(unix))]
fn create_private_directory(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)
}

fn create_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(KEY_FILE_MODE);
    options.open(path)
}

fn persist(mut file: File, contents: &[u8]) -> io::Result<()> {
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> io::Result<()> {
    File::open(directory)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> io::Result<()> {
    Ok(())
}

fn environment_prefix(application: &str) -> String {
    application
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const APPLICATION: &str = "example";

    fn environment(variables: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let variables: HashMap<String, OsString> = variables
            .iter()
            .map(|(name, value)| (name.to_string(), OsString::from(value)))
            .collect();
        move |name| variables.get(name).cloned()
    }

    #[test]
    fn debug_is_redacted() {
        let key = MasterKey::new([7u8; KEY_BYTES]);
        assert_eq!(format!("{key:?}"), "MasterKey([REDACTED])");
    }

    #[test]
    fn generated_keys_differ() {
        assert_ne!(
            MasterKey::generate().unwrap().as_bytes(),
            MasterKey::generate().unwrap().as_bytes()
        );
    }

    #[test]
    fn hexadecimal_round_trips() {
        let key = MasterKey::generate().unwrap();
        let hexadecimal: Zeroizing<String> = key.to_hex();
        let restored = MasterKey::from_hex(&hexadecimal).unwrap();
        assert_eq!(key.as_bytes(), restored.as_bytes());
    }

    #[test]
    fn hexadecimal_is_trimmed() {
        let key = MasterKey::generate().unwrap();
        let padded = format!("  {}\n", key.to_hex().as_str());
        assert_eq!(
            MasterKey::from_hex(&padded).unwrap().as_bytes(),
            key.as_bytes()
        );
    }

    #[test]
    fn rejects_the_wrong_key_length() {
        assert!(matches!(
            MasterKey::from_hex("abcdef"),
            Err(SecretError::KeyLength {
                expected: 32,
                actual: 3
            })
        ));
    }

    #[test]
    fn rejects_characters_that_are_not_hexadecimal() {
        assert!(matches!(
            MasterKey::from_hex("not_valid_hex_string_of_64_chars_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"),
            Err(SecretError::InvalidHexadecimal)
        ));
    }

    #[test]
    fn a_written_key_file_reads_back() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("nested").join(KEY_FILE_NAME);

        let original = MasterKey::generate().unwrap();
        write_key_file(&path, &original).unwrap();

        assert_eq!(
            read_key_file(&path).unwrap().as_bytes(),
            original.as_bytes()
        );
    }

    #[test]
    fn a_rewritten_key_file_holds_the_new_key() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join(KEY_FILE_NAME);

        write_key_file(&path, &MasterKey::generate().unwrap()).unwrap();
        let replacement = MasterKey::generate().unwrap();
        write_key_file(&path, &replacement).unwrap();

        assert_eq!(
            read_key_file(&path).unwrap().as_bytes(),
            replacement.as_bytes()
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_directory_a_key_file_is_renamed_into_is_opened_and_synced() {
        let directory = tempfile::TempDir::new().unwrap();
        sync_directory(directory.path()).unwrap();

        let missing = sync_directory(&directory.path().join("missing")).unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn writing_a_key_file_leaves_nothing_else_behind() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join(KEY_FILE_NAME);
        write_key_file(&path, &MasterKey::generate().unwrap()).unwrap();

        let entries: Vec<PathBuf> = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(entries, vec![path]);
    }

    #[cfg(unix)]
    #[test]
    fn a_written_key_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join(KEY_FILE_NAME);
        write_key_file(&path, &MasterKey::generate().unwrap()).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, KEY_FILE_MODE, "key file is world readable");
    }

    #[cfg(unix)]
    #[test]
    fn missing_parent_directories_are_created_readable_only_by_their_owner() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::TempDir::new().unwrap();
        let outer = directory.path().join("outer");
        let inner = outer.join("inner");
        write_key_file(&inner.join(KEY_FILE_NAME), &MasterKey::generate().unwrap()).unwrap();

        for created in [outer, inner] {
            let mode = fs::metadata(&created).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                KEY_DIRECTORY_MODE,
                "{created:?} is open to other users"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_file_already_at_the_path_never_receives_the_key() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join(KEY_FILE_NAME);
        let witness = directory.path().join("witness");
        fs::write(&path, "previous contents").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        fs::hard_link(&path, &witness).unwrap();

        let key = MasterKey::generate().unwrap();
        write_key_file(&path, &key).unwrap();

        assert_eq!(
            fs::read_to_string(&witness).unwrap(),
            "previous contents",
            "the key was written into the world-readable file that was already there"
        );
        assert_eq!(read_key_file(&path).unwrap().as_bytes(), key.as_bytes());
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_at_the_path_is_replaced_not_followed() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join(KEY_FILE_NAME);
        let target = directory.path().join("target");
        fs::write(&target, "untouched").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();

        let key = MasterKey::generate().unwrap();
        write_key_file(&path, &key).unwrap();

        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "untouched",
            "the key was written through the symbolic link"
        );
        assert!(
            !fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(read_key_file(&path).unwrap().as_bytes(), key.as_bytes());
    }

    #[test]
    fn reading_a_missing_key_file_names_it() {
        let path = Path::new("/nonexistent/abnegate/master.key");
        assert!(matches!(
            read_key_file(path),
            Err(SecretError::ReadKeyFile { path: reported, .. }) if reported == path
        ));
    }

    #[test]
    fn no_configured_key_means_plaintext_mode() {
        let home = tempfile::TempDir::new().unwrap();
        let absent =
            load_master_key_from(APPLICATION, environment(&[]), Some(home.path().into())).unwrap();
        assert!(absent.is_none());
    }

    #[test]
    fn the_key_variable_is_read() {
        let key = MasterKey::generate().unwrap();
        let loaded = load_master_key_from(
            APPLICATION,
            environment(&[("EXAMPLE_MASTER_KEY", key.to_hex().as_str())]),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(loaded.as_bytes(), key.as_bytes());
    }

    #[test]
    fn the_key_file_variable_is_read() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join(KEY_FILE_NAME);
        let key = MasterKey::generate().unwrap();
        write_key_file(&path, &key).unwrap();

        let loaded = load_master_key_from(
            APPLICATION,
            environment(&[("EXAMPLE_MASTER_KEY_FILE", path.to_str().unwrap())]),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(loaded.as_bytes(), key.as_bytes());
    }

    #[test]
    fn the_default_path_under_the_home_directory_is_read() {
        let home = tempfile::TempDir::new().unwrap();
        let key = MasterKey::generate().unwrap();
        write_key_file(&key_path_under(home.path(), APPLICATION), &key).unwrap();

        let loaded = load_master_key_from(APPLICATION, environment(&[]), Some(home.path().into()))
            .unwrap()
            .unwrap();
        assert_eq!(loaded.as_bytes(), key.as_bytes());
    }

    #[test]
    fn without_a_home_directory_the_key_is_not_looked_for_in_the_working_directory() {
        assert!(matches!(
            load_master_key_from(APPLICATION, environment(&[]), None),
            Err(SecretError::NoHomeDirectory)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_key_variable_that_is_not_utf8_is_an_error() {
        use std::os::unix::ffi::OsStringExt;

        let invalid = |name: &str| {
            (name == "EXAMPLE_MASTER_KEY").then(|| OsString::from_vec(vec![b'a', 0xFF, b'b']))
        };
        let home = tempfile::TempDir::new().unwrap();
        assert!(matches!(
            load_master_key_from(APPLICATION, invalid, Some(home.path().into())),
            Err(SecretError::InvalidEnvironment { variable }) if variable == "EXAMPLE_MASTER_KEY"
        ));
    }

    #[test]
    fn the_default_key_path_is_under_a_dot_directory_named_for_the_application() {
        let path = key_path_under(Path::new("/home/user"), APPLICATION);
        assert_eq!(path, Path::new("/home/user/.example").join(KEY_FILE_NAME));
    }

    #[test]
    fn the_environment_prefix_is_a_valid_variable_name() {
        assert_eq!(environment_prefix("example"), "EXAMPLE");
        assert_eq!(environment_prefix("my-app"), "MY_APP");
        assert_eq!(environment_prefix("my.app"), "MY_APP");
    }
}
