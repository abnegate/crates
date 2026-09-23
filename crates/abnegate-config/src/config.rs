use std::fs;
use std::path::{Path, PathBuf};

use abnegate_secret::MasterKey;
use serde::Serialize;
use serde::de::DeserializeOwned;
use toml::Value;

use crate::envelope::{self, Location};
use crate::error::ConfigError;
use crate::loader::Loader;

/// An application's settings together with the file they came from.
///
/// `T` is the application's own type; this crate never names its fields.
#[derive(Debug)]
pub struct Config<T> {
    path: PathBuf,
    value: T,
    sealed: Vec<Location>,
}

impl<T> Config<T> {
    /// Settings that will be written to `path`, with nothing sealed.
    pub fn new(path: impl Into<PathBuf>, value: T) -> Self {
        Self {
            path: path.into(),
            value,
            sealed: Vec::new(),
        }
    }

    pub(crate) fn loaded(path: PathBuf, value: T, sealed: Vec<Location>) -> Self {
        Self {
            path,
            value,
            sealed,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn value_mut(&mut self) -> &mut T {
        &mut self.value
    }

    pub fn into_value(self) -> T {
        self.value
    }
}

impl<T: DeserializeOwned> Config<T> {
    /// Load `application`'s settings from the conventional location.
    ///
    /// Fails with [`ConfigError::Missing`] when the file is not there.
    pub fn load(application: &str) -> Result<Self, ConfigError> {
        Loader::new(application)?.load()
    }
}

impl<T: DeserializeOwned + Default> Config<T> {
    /// Load `application`'s settings, falling back to [`Default`] when the file
    /// is not there. Nothing is written until [`Config::save`] is called.
    pub fn load_or_default(application: &str) -> Result<Self, ConfigError> {
        Loader::new(application)?.load_or_default()
    }
}

impl<T: Serialize> Config<T> {
    /// Write the settings out, creating the directory if it is missing.
    ///
    /// Values that arrived sealed are written back in plaintext; use
    /// [`Config::save_sealed`] to keep them encrypted.
    pub fn save(&self) -> Result<(), ConfigError> {
        self.write(self.document()?)
    }

    /// Write the settings out, resealing every value that was sealed on load.
    pub fn save_sealed(&self, key: &MasterKey) -> Result<(), ConfigError> {
        let mut document = self.document()?;
        envelope::seal(&mut document, &self.sealed, key)?;
        self.write(document)
    }

    fn document(&self) -> Result<Value, ConfigError> {
        Ok(Value::try_from(&self.value)?)
    }

    fn write(&self, document: Value) -> Result<(), ConfigError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
                path: self.path.clone(),
                source,
            })?;
        }

        fs::write(&self.path, toml::to_string_pretty(&document)?).map_err(|source| {
            ConfigError::Write {
                path: self.path.clone(),
                source,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use abnegate_secret::{SecretValue, encrypt_value};
    use serde::Deserialize;
    use tempfile::TempDir;

    use super::*;

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct Settings {
        #[serde(default = "default_model")]
        model: String,
        host: Option<String>,
        #[serde(default = "default_iterations")]
        iterations: u32,
        #[serde(default = "default_editor")]
        editor: String,
    }

    fn default_model() -> String {
        "gpt-4o".to_string()
    }

    fn default_iterations() -> u32 {
        50
    }

    fn default_editor() -> String {
        "vim".to_string()
    }

    impl Default for Settings {
        fn default() -> Self {
            Self {
                model: default_model(),
                host: None,
                iterations: default_iterations(),
                editor: default_editor(),
            }
        }
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct Credentials {
        password: String,
    }

    fn settings() -> Settings {
        Settings {
            model: "gpt-4".to_string(),
            host: Some("https://example.com".to_string()),
            iterations: 100,
            editor: "nano".to_string(),
        }
    }

    #[test]
    fn defaults_fill_in_what_the_file_omits() {
        let settings: Settings = toml::from_str("").unwrap();

        assert_eq!(settings.model, "gpt-4o");
        assert!(settings.host.is_none());
        assert_eq!(settings.iterations, 50);
        assert!(!settings.editor.is_empty());
    }

    #[test]
    fn a_partial_file_keeps_the_rest_of_the_defaults() {
        let settings: Settings = toml::from_str("model = \"custom-model\"").unwrap();

        assert_eq!(settings.model, "custom-model");
        assert!(settings.host.is_none());
        assert_eq!(settings.iterations, 50);
    }

    #[test]
    fn settings_serialize() {
        let serialized = toml::to_string(&settings()).unwrap();

        assert!(serialized.contains("gpt-4"));
        assert!(serialized.contains("example.com"));
        assert!(serialized.contains("100"));
        assert!(serialized.contains("nano"));
    }

    #[test]
    fn settings_deserialize() {
        let settings: Settings = toml::from_str(
            r#"
            model = "claude-3"
            host = "https://api.example.com"
            iterations = 25
            editor = "code"
            "#,
        )
        .unwrap();

        assert_eq!(settings.model, "claude-3");
        assert_eq!(settings.host, Some("https://api.example.com".to_string()));
        assert_eq!(settings.iterations, 25);
        assert_eq!(settings.editor, "code");
    }

    #[test]
    fn settings_round_trip_through_toml() {
        let original = settings();

        let restored: Settings =
            toml::from_str(&toml::to_string_pretty(&original).unwrap()).unwrap();

        assert_eq!(original, restored);
    }

    #[test]
    fn an_absent_option_stays_absent() {
        let original = Settings {
            host: None,
            ..settings()
        };

        let restored: Settings = toml::from_str(&toml::to_string(&original).unwrap()).unwrap();

        assert!(restored.host.is_none());
    }

    #[test]
    fn every_iteration_count_round_trips() {
        for iterations in [1, 10, 50, 100, 1000] {
            let original = Settings {
                iterations,
                ..settings()
            };

            let restored: Settings = toml::from_str(&toml::to_string(&original).unwrap()).unwrap();

            assert_eq!(restored.iterations, iterations);
        }
    }

    #[test]
    fn settings_clone() {
        let original = settings();

        assert_eq!(original.clone(), original);
    }

    #[test]
    fn settings_are_debuggable() {
        let debugged = format!("{:?}", Settings::default());

        assert!(debugged.contains("Settings"));
        assert!(debugged.contains("model"));
    }

    #[test]
    fn what_is_saved_is_what_is_loaded() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("config.toml");

        Config::new(&path, settings()).save().unwrap();

        let loaded = Loader::at(&path).load::<Settings>().unwrap();
        assert_eq!(loaded.value(), &settings());
        assert_eq!(loaded.path(), path);
    }

    #[test]
    fn saving_creates_the_directory() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("nested").join("config.toml");

        Config::new(&path, Settings::default()).save().unwrap();

        assert!(path.exists());
    }

    #[test]
    fn settings_can_be_edited_in_place_and_written_back() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("config.toml");
        let mut config = Config::new(&path, Settings::default());

        config.value_mut().host = Some("https://example.com".to_string());
        config.save().unwrap();

        let loaded = Loader::at(&path).load::<Settings>().unwrap();
        assert_eq!(
            loaded.into_value().host,
            Some("https://example.com".to_string())
        );
    }

    #[test]
    fn an_application_without_a_configuration_file_reports_it_missing() {
        let error = Config::<Settings>::load("abnegate-config-no-such-application").unwrap_err();

        assert!(matches!(error, ConfigError::Missing { .. }), "{error:?}");
    }

    #[test]
    fn an_application_without_a_configuration_file_still_has_defaults() {
        let config =
            Config::<Settings>::load_or_default("abnegate-config-no-such-application").unwrap();

        assert_eq!(config.value(), &Settings::default());
    }

    #[test]
    fn resealing_returns_a_sealed_value_to_the_file() {
        let key = MasterKey::generate();
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("config.toml");
        let envelope = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        fs::write(&path, format!("password = \"{envelope}\"\n")).unwrap();

        let config = Loader::at(&path)
            .master_key(&key)
            .load::<Credentials>()
            .unwrap();
        config.save_sealed(&key).unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(!written.contains("hunter2"), "{written}");
        assert_eq!(
            Loader::at(&path)
                .master_key(&key)
                .load::<Credentials>()
                .unwrap()
                .value()
                .password,
            "hunter2"
        );
    }

    #[test]
    fn resealing_an_edited_value_seals_the_new_one() {
        let key = MasterKey::generate();
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("config.toml");
        let envelope = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        fs::write(&path, format!("password = \"{envelope}\"\n")).unwrap();

        let mut config = Loader::at(&path)
            .master_key(&key)
            .load::<Credentials>()
            .unwrap();
        config.value_mut().password = "correct-horse".to_string();
        config.save_sealed(&key).unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(!written.contains("correct-horse"), "{written}");
        assert_eq!(
            Loader::at(&path)
                .master_key(&key)
                .load::<Credentials>()
                .unwrap()
                .value()
                .password,
            "correct-horse"
        );
    }

    #[test]
    fn resealing_leaves_a_value_that_never_arrived_sealed_alone() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("config.toml");

        Config::new(
            &path,
            Credentials {
                password: "hunter2".to_string(),
            },
        )
        .save_sealed(&MasterKey::generate())
        .unwrap();

        assert!(fs::read_to_string(&path).unwrap().contains("hunter2"));
    }
}
