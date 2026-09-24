use std::fs;
use std::path::Path;
use std::path::PathBuf;

use abnegate_secret::MasterKey;
use serde::de::DeserializeOwned;
use serde::de::Error as _;
use toml::Value;

use crate::application::Application;
use crate::config::Config;
use crate::envelope;
use crate::error::Error;
use crate::path::path;

/// Where a configuration file lives and how to unseal it.
///
/// Values written as `ENC[v1:...]` envelopes are decrypted on load once a
/// master key is given; without one they are handed to the application exactly
/// as they were written.
pub struct Loader<'key> {
    path: PathBuf,
    key: Option<&'key MasterKey>,
}

impl<'key> Loader<'key> {
    /// Load from the conventional location for `application`.
    pub fn new(application: &Application) -> Result<Self, Error> {
        Ok(Self::at(path(application)?))
    }

    /// Load from an exact path.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            key: None,
        }
    }

    /// Decrypt sealed values with `key` as they are read.
    ///
    /// The [`Config`] that comes back keeps its own copy of the key when
    /// anything was sealed, so [`Config::save`] can seal it again.
    pub fn master_key(mut self, key: &'key MasterKey) -> Self {
        self.key = Some(key);
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        self.path.is_file()
    }

    /// Read the file, failing with [`Error::Missing`] when it is absent.
    pub fn load<T: DeserializeOwned>(&self) -> Result<Config<T>, Error> {
        if !self.exists() {
            return Err(Error::Missing {
                path: self.path.clone(),
            });
        }

        self.read()
    }

    /// Read the file, falling back to [`Default`] when it is absent.
    pub fn load_or_default<T: DeserializeOwned + Default>(&self) -> Result<Config<T>, Error> {
        if !self.exists() {
            return Ok(Config::new(self.path.clone(), T::default()));
        }

        self.read()
    }

    fn read<T: DeserializeOwned>(&self) -> Result<Config<T>, Error> {
        let content = fs::read_to_string(&self.path).map_err(|source| Error::Read {
            path: self.path.clone(),
            source,
        })?;

        let mut document: Value = toml::from_str(&content).map_err(|source| Error::Parse {
            path: self.path.clone(),
            source,
        })?;

        let sealed = envelope::unseal(&mut document, self.key)?;
        let key = self.key.filter(|_| !sealed.is_empty()).and_then(duplicate);

        let value = document.try_into().map_err(|source| Error::Parse {
            path: self.path.clone(),
            source: if sealed.is_empty() {
                source
            } else {
                rejection_as_written::<T>(&content)
            },
        })?;

        Ok(Config::loaded(self.path.clone(), value, key, sealed))
    }
}

fn duplicate(key: &MasterKey) -> Option<MasterKey> {
    MasterKey::from_hex(&key.to_hex()).ok()
}

/// Why `content` does not fit `T`, told from the file as it was written so a
/// decrypted value never reaches the message.
fn rejection_as_written<T: DeserializeOwned>(content: &str) -> toml::de::Error {
    match toml::from_str::<T>(content) {
        Err(rejection) => rejection,
        Ok(_) => toml::de::Error::custom("a sealed value does not fit the settings once decrypted"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error as _;

    use abnegate_secret::SecretValue;
    use abnegate_secret::encrypt_value;
    use abnegate_secret::is_encrypted;
    use serde::Deserialize;
    use serde::Deserializer;
    use serde::Serialize;
    use serde::de::Unexpected;
    use tempfile::TempDir;

    use super::*;

    #[derive(Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
    struct Settings {
        #[serde(default)]
        model: String,
        #[serde(default)]
        password: String,
    }

    type Ports = BTreeMap<String, u32>;

    #[derive(Debug)]
    struct OnlySealed;

    impl<'de> Deserialize<'de> for OnlySealed {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let text = String::deserialize(deserializer)?;
            if is_encrypted(&text) {
                Ok(Self)
            } else {
                Err(D::Error::invalid_value(
                    Unexpected::Str(&text),
                    &"an envelope",
                ))
            }
        }
    }

    fn rendered(error: &Error) -> String {
        let mut rendered = format!("{error}\n{error:?}\n");
        let mut source = error.source();
        while let Some(cause) = source {
            rendered.push_str(&format!("{cause}\n{cause:?}\n"));
            source = cause.source();
        }
        rendered
    }

    fn written(content: &str) -> (TempDir, PathBuf) {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, content).unwrap();
        (directory, path)
    }

    #[test]
    fn a_loader_for_an_application_points_at_its_configuration_file() {
        let loader = Loader::new(&Application::new("example").unwrap()).unwrap();

        assert!(
            loader.path().ends_with("config.toml"),
            "{:?}",
            loader.path()
        );
    }

    #[test]
    fn a_missing_file_does_not_exist() {
        let directory = TempDir::new().unwrap();

        assert!(!Loader::at(directory.path().join("config.toml")).exists());
    }

    #[test]
    fn a_directory_is_not_a_configuration_file() {
        let directory = TempDir::new().unwrap();

        assert!(!Loader::at(directory.path()).exists());
    }

    #[test]
    fn loading_a_missing_file_reports_the_path() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("config.toml");

        let error = Loader::at(&path).load::<Settings>().unwrap_err();

        assert!(
            matches!(&error, Error::Missing { path: reported } if reported == &path),
            "{error:?}"
        );
    }

    #[test]
    fn a_missing_file_loads_as_the_default() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("config.toml");

        let config = Loader::at(&path).load_or_default::<Settings>().unwrap();

        assert_eq!(config.value(), &Settings::default());
        assert_eq!(config.path(), path);
    }

    #[test]
    fn an_existing_file_wins_over_the_default() {
        let (_directory, path) = written("model = \"gpt-4o\"\n");

        let config = Loader::at(&path).load_or_default::<Settings>().unwrap();

        assert_eq!(config.value().model, "gpt-4o");
    }

    #[test]
    fn a_malformed_file_reports_the_path() {
        let (_directory, path) = written("invalid { toml");

        let error = Loader::at(&path).load::<Settings>().unwrap_err();

        assert!(
            matches!(&error, Error::Parse { path: reported, .. } if reported == &path),
            "{error:?}"
        );
    }

    #[test]
    fn a_file_of_the_wrong_shape_reports_the_path() {
        let (_directory, path) = written("model = 12\n");

        let error = Loader::at(&path).load::<Settings>().unwrap_err();

        assert!(matches!(error, Error::Parse { .. }), "{error:?}");
    }

    #[test]
    fn a_sealed_value_arrives_as_plaintext() {
        let key = MasterKey::generate().unwrap();
        let envelope = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let (_directory, path) = written(&format!("password = \"{envelope}\"\n"));

        let config = Loader::at(&path)
            .master_key(&key)
            .load::<Settings>()
            .unwrap();

        assert_eq!(config.value().password, "hunter2");
    }

    #[test]
    fn a_sealed_value_stays_sealed_without_a_key() {
        let key = MasterKey::generate().unwrap();
        let envelope = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let (_directory, path) = written(&format!("password = \"{envelope}\"\n"));

        let config = Loader::at(&path).load::<Settings>().unwrap();

        assert!(is_encrypted(&config.value().password));
    }

    #[test]
    fn the_wrong_key_fails_the_load() {
        let envelope = encrypt_value(
            &SecretValue::new("hunter2"),
            &MasterKey::generate().unwrap(),
        )
        .unwrap();
        let (_directory, path) = written(&format!("password = \"{envelope}\"\n"));

        let error = Loader::at(&path)
            .master_key(&MasterKey::generate().unwrap())
            .load::<Settings>()
            .unwrap_err();

        assert!(
            matches!(&error, Error::Decrypt { field, .. } if field == "password"),
            "{error:?}"
        );
    }

    #[test]
    fn a_sealed_value_of_the_wrong_type_is_reported_without_its_plaintext() {
        let key = MasterKey::generate().unwrap();
        let envelope = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let (_directory, path) = written(&format!("port = \"{envelope}\"\n"));

        let error = Loader::at(&path)
            .master_key(&key)
            .load::<Ports>()
            .unwrap_err();

        let rendered = rendered(&error);
        assert!(
            matches!(&error, Error::Parse { path: reported, .. } if reported == &path),
            "{rendered}"
        );
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(rendered.contains("port"), "{rendered}");
    }

    #[test]
    fn a_sealed_value_only_its_plaintext_fails_is_reported_without_it() {
        let key = MasterKey::generate().unwrap();
        let envelope = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let (_directory, path) = written(&format!("password = \"{envelope}\"\n"));

        let error = Loader::at(&path)
            .master_key(&key)
            .load::<BTreeMap<String, OnlySealed>>()
            .unwrap_err();

        let rendered = rendered(&error);
        assert!(matches!(error, Error::Parse { .. }), "{rendered}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
    }

    #[test]
    fn a_file_without_envelopes_keeps_its_own_parse_error() {
        let (_directory, path) = written("port = \"eighty\"\n");

        let error = Loader::at(&path)
            .master_key(&MasterKey::generate().unwrap())
            .load::<Ports>()
            .unwrap_err();

        assert!(rendered(&error).contains("eighty"), "{error:?}");
    }

    #[test]
    fn a_key_changes_nothing_for_a_file_without_envelopes() {
        let (_directory, path) = written("model = \"gpt-4o\"\npassword = \"plain\"\n");

        let config = Loader::at(&path)
            .master_key(&MasterKey::generate().unwrap())
            .load::<Settings>()
            .unwrap();

        assert_eq!(config.value().password, "plain");
    }
}
