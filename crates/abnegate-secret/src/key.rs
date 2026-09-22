use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use zeroize::Zeroize;

use crate::error::SecretError;
use crate::redact::REDACTED;

pub(crate) const KEY_BYTES: usize = 32;

const KEY_FILE_NAME: &str = "master.key";
const KEY_FILE_MODE: u32 = 0o600;

/// The AES-256 key every envelope in a deployment is sealed with.
pub struct MasterKey {
    key: [u8; KEY_BYTES],
}

impl MasterKey {
    pub fn new(key: [u8; KEY_BYTES]) -> Self {
        Self { key }
    }

    pub fn generate() -> Self {
        Self {
            key: rand::random(),
        }
    }

    pub fn from_hex(hexadecimal: &str) -> Result<Self, SecretError> {
        let mut decoded =
            hex::decode(hexadecimal.trim()).map_err(|_| SecretError::InvalidHexadecimal)?;
        if decoded.len() != KEY_BYTES {
            let actual = decoded.len();
            decoded.zeroize();
            return Err(SecretError::KeyLength {
                expected: KEY_BYTES,
                actual,
            });
        }

        let mut key = [0u8; KEY_BYTES];
        key.copy_from_slice(&decoded);
        decoded.zeroize();
        Ok(Self { key })
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.key)
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

/// Where `application` keeps its master key when no other source names one.
pub fn default_key_path(application: &str) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(format!(".{application}"))
        .join(KEY_FILE_NAME)
}

/// The master key `application` is configured with, in order of precedence:
/// the hexadecimal `<APPLICATION>_MASTER_KEY` variable, the path in
/// `<APPLICATION>_MASTER_KEY_FILE`, then [`default_key_path`].
///
/// `None` means no key is configured, which is the caller's cue to run in
/// plaintext mode.
pub fn load_master_key(application: &str) -> Result<Option<MasterKey>, SecretError> {
    let prefix = environment_prefix(application);

    if let Ok(hexadecimal) = env::var(format!("{prefix}_MASTER_KEY"))
        && !hexadecimal.is_empty()
    {
        return MasterKey::from_hex(&hexadecimal).map(Some);
    }

    if let Ok(path) = env::var(format!("{prefix}_MASTER_KEY_FILE"))
        && !path.is_empty()
    {
        return read_key_file(Path::new(&path)).map(Some);
    }

    let path = default_key_path(application);
    if path.exists() {
        return read_key_file(&path).map(Some);
    }

    Ok(None)
}

/// Read a hexadecimal master key from `path`.
pub fn read_key_file(path: &Path) -> Result<MasterKey, SecretError> {
    let mut content = fs::read_to_string(path).map_err(|source| SecretError::ReadKeyFile {
        path: path.to_path_buf(),
        source,
    })?;
    let key = MasterKey::from_hex(&content);
    content.zeroize();
    key
}

/// Write `key` to `path` as hexadecimal, readable only by its owner.
pub fn write_key_file(path: &Path, key: &MasterKey) -> Result<(), SecretError> {
    let failed = |source: std::io::Error| SecretError::WriteKeyFile {
        path: path.to_path_buf(),
        source,
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(failed)?;
    }

    let mut hexadecimal = key.to_hex();
    let written = fs::write(path, &hexadecimal);
    hexadecimal.zeroize();
    written.map_err(failed)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(KEY_FILE_MODE)).map_err(failed)?;
    }

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
    use super::*;

    #[test]
    fn debug_is_redacted() {
        let key = MasterKey::new([7u8; KEY_BYTES]);
        assert_eq!(format!("{key:?}"), "MasterKey([REDACTED])");
    }

    #[test]
    fn generated_keys_differ() {
        assert_ne!(
            MasterKey::generate().as_bytes(),
            MasterKey::generate().as_bytes()
        );
    }

    #[test]
    fn hexadecimal_round_trips() {
        let key = MasterKey::generate();
        let restored = MasterKey::from_hex(&key.to_hex()).unwrap();
        assert_eq!(key.as_bytes(), restored.as_bytes());
    }

    #[test]
    fn hexadecimal_is_trimmed() {
        let key = MasterKey::generate();
        let padded = format!("  {}\n", key.to_hex());
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

        let original = MasterKey::generate();
        write_key_file(&path, &original).unwrap();

        assert_eq!(
            read_key_file(&path).unwrap().as_bytes(),
            original.as_bytes()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_written_key_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join(KEY_FILE_NAME);
        write_key_file(&path, &MasterKey::generate()).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, KEY_FILE_MODE, "key file is world readable");
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
        let absent = load_master_key("abnegate-secret-no-such-application").unwrap();
        assert!(absent.is_none());
    }

    #[test]
    fn the_default_key_path_is_under_a_dot_directory_named_for_the_application() {
        let path = default_key_path("example");
        assert!(
            path.ends_with(Path::new(".example").join(KEY_FILE_NAME)),
            "{path:?}"
        );
    }

    #[test]
    fn the_environment_prefix_is_a_valid_variable_name() {
        assert_eq!(environment_prefix("example"), "EXAMPLE");
        assert_eq!(environment_prefix("my-app"), "MY_APP");
        assert_eq!(environment_prefix("my.app"), "MY_APP");
    }
}
