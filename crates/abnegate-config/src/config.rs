use std::path::Path;
use std::path::PathBuf;

use abnegate_secret::MasterKey;
use serde::Serialize;
use serde::de::DeserializeOwned;
use toml::Value;

use crate::application::Application;
use crate::envelope;
use crate::envelope::Sealed;
use crate::error::Error;
use crate::loader::Loader;
use crate::private_file::PrivateFile;

/// An application's settings together with the file they came from.
///
/// `T` is the application's own type; this crate never names its fields.
#[derive(Debug)]
pub struct Config<T> {
    path: PathBuf,
    value: T,
    key: Option<MasterKey>,
    sealed: Vec<Sealed>,
}

impl<T> Config<T> {
    /// Settings that will be written to `path`, with nothing sealed.
    pub fn new(path: impl Into<PathBuf>, value: T) -> Self {
        Self {
            path: path.into(),
            value,
            key: None,
            sealed: Vec::new(),
        }
    }

    pub(crate) fn loaded(
        path: PathBuf,
        value: T,
        key: Option<MasterKey>,
        sealed: Vec<Sealed>,
    ) -> Self {
        Self {
            path,
            value,
            key,
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
    /// Fails with [`Error::Missing`] when the file is not there.
    pub fn load(application: &Application) -> Result<Self, Error> {
        Loader::new(application)?.load()
    }
}

impl<T: DeserializeOwned + Default> Config<T> {
    /// Load `application`'s settings, falling back to [`Default`] when the file
    /// is not there. Nothing is written until [`Config::save`] is called.
    pub fn load_or_default(application: &Application) -> Result<Self, Error> {
        Loader::new(application)?.load_or_default()
    }
}

impl<T: Serialize> Config<T> {
    /// Write the settings out, sealing again every value that arrived sealed.
    ///
    /// A value is sealed wherever it now appears, so one read under a new name
    /// through a serde alias, moved to a new map key, or shifted within an
    /// array by removing other elements stays sealed. When an array on its key
    /// path changed in any other way, an element added, reordered or edited,
    /// every string on that key path is sealed, plain neighbours included: an
    /// edited secret may be any of them, and a neighbour sealed needlessly
    /// still reads back as it was.
    ///
    /// Values are sealed with the key the [`Loader`] was given. Without one, a
    /// value that still holds its envelope is written as it was, and one that
    /// would be written in the clear, a neighbour that has to be sealed among
    /// them, fails with [`Error::SealedWithoutKey`] rather than reach the
    /// disk. A sealed value whose location is gone fails with
    /// [`Error::SealedShapeChanged`] when fewer strings in the settings
    /// hold it than the file did, or when it was empty, since it cannot be told
    /// apart from one that moved to a new key and was edited on the way.
    ///
    /// The file is replaced atomically and is readable only by its owner; a
    /// directory created for it is too.
    pub fn save(&self) -> Result<(), Error> {
        self.write(self.key.as_ref())
    }

    /// Write the settings out, sealing every value that arrived sealed with
    /// `key` instead of the key they were loaded with.
    pub fn save_sealed(&self, key: &MasterKey) -> Result<(), Error> {
        self.write(Some(key))
    }

    fn write(&self, key: Option<&MasterKey>) -> Result<(), Error> {
        let mut document = Value::try_from(&self.value)?;
        envelope::seal(&mut document, &self.sealed, key)?;

        PrivateFile::new(&self.path).write(toml::to_string_pretty(&document)?.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use abnegate_secret::SecretValue;
    use abnegate_secret::encrypt_value;
    use serde::Deserialize;
    use tempfile::TempDir;

    use super::*;

    const SEALED: &str = "ENC[v1:";

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

    #[derive(Debug, Deserialize, Serialize)]
    struct Renamed {
        #[serde(alias = "api_key")]
        token: String,
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct Mirrored {
        #[serde(alias = "api_key")]
        token: String,
        backup: String,
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct Proxied {
        #[serde(alias = "api_key")]
        token: String,
        #[serde(default)]
        proxy: String,
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct Profiles {
        profiles: BTreeMap<String, Credentials>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct Fleet {
        servers: Vec<Server>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct Server {
        name: String,
        password: String,
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct Account {
        name: String,
        password: String,
        hosts: Vec<String>,
    }

    fn absent() -> Application {
        Application::new("abnegate-config-no-such-application").unwrap()
    }

    fn sealed_file(content: impl FnOnce(&str) -> String, key: &MasterKey) -> (TempDir, PathBuf) {
        sealed_file_holding("hunter2", content, key)
    }

    fn sealed_file_holding(
        secret: &str,
        content: impl FnOnce(&str) -> String,
        key: &MasterKey,
    ) -> (TempDir, PathBuf) {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("config.toml");
        let envelope = encrypt_value(&SecretValue::new(secret), key).unwrap();
        fs::write(&path, content(&envelope)).unwrap();
        (directory, path)
    }

    fn account(envelope: &str) -> String {
        format!(
            "name = \"person\"\npassword = \"{envelope}\"\nhosts = [\"one\", \"{envelope}\", \"three\"]\n"
        )
    }

    fn fleet(envelope: &str) -> String {
        format!(
            "[[servers]]\nname = \"a\"\npassword = \"{envelope}\"\n\n[[servers]]\nname = \"b\"\npassword = \"{envelope}\"\n"
        )
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
        let error = Config::<Settings>::load(&absent()).unwrap_err();

        assert!(matches!(error, Error::Missing { .. }), "{error:?}");
    }

    #[test]
    fn an_application_without_a_configuration_file_still_has_defaults() {
        let config = Config::<Settings>::load_or_default(&absent()).unwrap();

        assert_eq!(config.value(), &Settings::default());
    }

    #[test]
    fn resealing_returns_a_sealed_value_to_the_file() {
        let key = MasterKey::generate().unwrap();
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
        let key = MasterKey::generate().unwrap();
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
        .save_sealed(&MasterKey::generate().unwrap())
        .unwrap();

        assert!(fs::read_to_string(&path).unwrap().contains("hunter2"));
    }

    #[test]
    fn saving_after_loading_with_a_key_keeps_every_secret_sealed() {
        let key = MasterKey::generate().unwrap();
        let (_directory, path) = sealed_file(account, &key);

        let mut config = Loader::at(&path)
            .master_key(&key)
            .load::<Account>()
            .unwrap();
        config.value_mut().name = "someone else".to_string();
        config.save().unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(!written.contains("hunter2"), "{written}");
        assert_eq!(written.matches(SEALED).count(), 2, "{written}");
        let reloaded = Loader::at(&path)
            .master_key(&key)
            .load::<Account>()
            .unwrap();
        assert_eq!(reloaded.value().name, "someone else");
        assert_eq!(reloaded.value().password, "hunter2");
        assert_eq!(reloaded.value().hosts, ["one", "hunter2", "three"]);
    }

    #[test]
    fn saving_after_an_array_shifts_seals_the_secret_where_it_moved() {
        let key = MasterKey::generate().unwrap();
        let (_directory, path) = sealed_file(account, &key);

        let mut config = Loader::at(&path)
            .master_key(&key)
            .load::<Account>()
            .unwrap();
        config.value_mut().hosts.remove(0);
        config.save().unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(!written.contains("hunter2"), "{written}");
        assert!(written.contains("\"three\""), "{written}");
        assert_eq!(
            Loader::at(&path)
                .master_key(&key)
                .load::<Account>()
                .unwrap()
                .value()
                .hosts,
            ["hunter2", "three"]
        );
    }

    #[test]
    fn saving_after_a_host_is_inserted_seals_every_host() {
        let key = MasterKey::generate().unwrap();
        let (_directory, path) = sealed_file(account, &key);

        let mut config = Loader::at(&path)
            .master_key(&key)
            .load::<Account>()
            .unwrap();
        config.value_mut().hosts.insert(0, "zero".to_string());
        config.save().unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(!written.contains("hunter2"), "{written}");
        assert!(!written.contains("\"zero\""), "{written}");
        assert_eq!(written.matches(SEALED).count(), 5, "{written}");
        assert_eq!(
            Loader::at(&path)
                .master_key(&key)
                .load::<Account>()
                .unwrap()
                .value()
                .hosts,
            ["zero", "one", "hunter2", "three"]
        );
    }

    #[test]
    fn saving_without_a_key_keeps_an_untouched_envelope() {
        let key = MasterKey::generate().unwrap();
        let (_directory, path) = sealed_file(account, &key);

        let mut config = Loader::at(&path).load::<Account>().unwrap();
        config.value_mut().name = "someone else".to_string();
        config.save().unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert_eq!(written.matches(SEALED).count(), 2, "{written}");
        assert_eq!(
            Loader::at(&path)
                .master_key(&key)
                .load::<Account>()
                .unwrap()
                .value()
                .password,
            "hunter2"
        );
    }

    #[test]
    fn saving_without_a_key_refuses_to_write_a_sealed_field_in_the_clear() {
        let key = MasterKey::generate().unwrap();
        let (_directory, path) = sealed_file(account, &key);
        let before = fs::read_to_string(&path).unwrap();

        let mut config = Loader::at(&path).load::<Account>().unwrap();
        config.value_mut().password = "correct-horse".to_string();
        let error = config.save().unwrap_err();

        assert!(
            matches!(&error, Error::SealedWithoutKey { field } if field == "password"),
            "{error:?}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn a_new_key_reseals_under_that_key() {
        let old = MasterKey::generate().unwrap();
        let new = MasterKey::generate().unwrap();
        let (_directory, path) = sealed_file(account, &old);

        Loader::at(&path)
            .master_key(&old)
            .load::<Account>()
            .unwrap()
            .save_sealed(&new)
            .unwrap();

        assert_eq!(
            Loader::at(&path)
                .master_key(&new)
                .load::<Account>()
                .unwrap()
                .value()
                .password,
            "hunter2"
        );
    }

    #[test]
    fn a_secret_read_through_an_alias_is_sealed_under_its_new_name() {
        let key = MasterKey::generate().unwrap();
        let (_directory, path) =
            sealed_file(|envelope| format!("api_key = \"{envelope}\"\n"), &key);

        Loader::at(&path)
            .master_key(&key)
            .load::<Renamed>()
            .unwrap()
            .save()
            .unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(!written.contains("hunter2"), "{written}");
        assert!(written.contains("token = \"ENC[v1:"), "{written}");
        assert_eq!(
            Loader::at(&path)
                .master_key(&key)
                .load::<Renamed>()
                .unwrap()
                .value()
                .token,
            "hunter2"
        );
    }

    #[test]
    fn a_secret_under_a_renamed_map_key_stays_sealed() {
        let key = MasterKey::generate().unwrap();
        let (_directory, path) = sealed_file(
            |envelope| format!("[profiles.default]\npassword = \"{envelope}\"\n"),
            &key,
        );

        let mut config = Loader::at(&path)
            .master_key(&key)
            .load::<Profiles>()
            .unwrap();
        let profiles = &mut config.value_mut().profiles;
        let profile = profiles.remove("default").unwrap();
        profiles.insert("work".to_string(), profile);
        config.save().unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(!written.contains("hunter2"), "{written}");
        assert_eq!(
            Loader::at(&path)
                .master_key(&key)
                .load::<Profiles>()
                .unwrap()
                .value()
                .profiles["work"]
                .password,
            "hunter2"
        );
    }

    #[test]
    fn a_secret_renamed_and_edited_at_once_is_refused() {
        let key = MasterKey::generate().unwrap();
        let (_directory, path) =
            sealed_file(|envelope| format!("api_key = \"{envelope}\"\n"), &key);
        let before = fs::read_to_string(&path).unwrap();

        let mut config = Loader::at(&path)
            .master_key(&key)
            .load::<Renamed>()
            .unwrap();
        config.value_mut().token = "correct-horse".to_string();
        let error = config.save().unwrap_err();

        assert!(
            matches!(&error, Error::SealedShapeChanged { field } if field == "api_key"),
            "{error:?}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn a_secret_renamed_and_edited_beside_a_sealed_copy_is_refused() {
        let key = MasterKey::generate().unwrap();
        let (_directory, path) = sealed_file(
            |envelope| format!("api_key = \"{envelope}\"\nbackup = \"{envelope}\"\n"),
            &key,
        );
        let before = fs::read_to_string(&path).unwrap();

        let mut config = Loader::at(&path)
            .master_key(&key)
            .load::<Mirrored>()
            .unwrap();
        config.value_mut().token = "correct-horse".to_string();
        let error = config.save().unwrap_err();

        assert!(
            matches!(&error, Error::SealedShapeChanged { field } if field == "api_key"),
            "{error:?}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn an_empty_secret_renamed_and_edited_beside_an_empty_string_is_refused() {
        let key = MasterKey::generate().unwrap();
        let (_directory, path) = sealed_file_holding(
            "",
            |envelope| format!("api_key = \"{envelope}\"\nproxy = \"\"\n"),
            &key,
        );
        let before = fs::read_to_string(&path).unwrap();

        let mut config = Loader::at(&path)
            .master_key(&key)
            .load::<Proxied>()
            .unwrap();
        config.value_mut().token = "correct-horse".to_string();
        let error = config.save().unwrap_err();

        assert!(
            matches!(&error, Error::SealedShapeChanged { field } if field == "api_key"),
            "{error:?}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn an_empty_secret_renamed_and_edited_beside_a_defaulted_field_is_refused() {
        let key = MasterKey::generate().unwrap();
        let (_directory, path) =
            sealed_file_holding("", |envelope| format!("api_key = \"{envelope}\"\n"), &key);
        let before = fs::read_to_string(&path).unwrap();

        let mut config = Loader::at(&path)
            .master_key(&key)
            .load::<Proxied>()
            .unwrap();
        config.value_mut().token = "correct-horse".to_string();
        let error = config.save().unwrap_err();

        assert!(
            matches!(&error, Error::SealedShapeChanged { field } if field == "api_key"),
            "{error:?}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn rotating_one_of_two_servers_that_share_a_secret_keeps_both_sealed() {
        let key = MasterKey::generate().unwrap();
        let (_directory, path) = sealed_file(fleet, &key);

        let mut config = Loader::at(&path).master_key(&key).load::<Fleet>().unwrap();
        config.value_mut().servers[1].password = "correct-horse".to_string();
        config.save().unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(!written.contains("hunter2"), "{written}");
        assert!(!written.contains("correct-horse"), "{written}");
        assert_eq!(written.matches(SEALED).count(), 2, "{written}");
        let servers = Loader::at(&path)
            .master_key(&key)
            .load::<Fleet>()
            .unwrap()
            .into_value()
            .servers;
        assert_eq!(servers[0].password, "hunter2");
        assert_eq!(servers[1].password, "correct-horse");
    }

    #[test]
    fn copying_a_server_then_rotating_the_original_keeps_both_sealed() {
        let key = MasterKey::generate().unwrap();
        let (_directory, path) = sealed_file(
            |envelope| format!("[[servers]]\nname = \"a\"\npassword = \"{envelope}\"\n"),
            &key,
        );

        let mut config = Loader::at(&path).master_key(&key).load::<Fleet>().unwrap();
        let servers = &mut config.value_mut().servers;
        servers.push(Server {
            name: "b".to_string(),
            password: servers[0].password.clone(),
        });
        servers[0].password = "correct-horse".to_string();
        config.save().unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(!written.contains("correct-horse"), "{written}");
        assert!(!written.contains("hunter2"), "{written}");
        let servers = Loader::at(&path)
            .master_key(&key)
            .load::<Fleet>()
            .unwrap()
            .into_value()
            .servers;
        assert_eq!(servers[0].password, "correct-horse");
        assert_eq!(servers[1].password, "hunter2");
    }

    #[cfg(unix)]
    #[test]
    fn a_saved_file_and_its_directory_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TempDir::new().unwrap();
        let nested = directory.path().join("nested");
        let path = nested.join("config.toml");

        Config::new(&path, settings()).save().unwrap();

        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&nested), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn saving_replaces_a_symlink_instead_of_writing_through_it() {
        let directory = TempDir::new().unwrap();
        let target = directory.path().join("elsewhere.toml");
        let path = directory.path().join("config.toml");
        fs::write(&target, "untouched = true\n").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();

        Config::new(&path, settings()).save().unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "untouched = true\n");
        assert_eq!(
            Loader::at(&path).load::<Settings>().unwrap().value(),
            &settings()
        );
    }
}
