use std::env;
use std::path::PathBuf;

use crate::application::Application;
use crate::error::ConfigError;

const DIRECTORY_VARIABLE: &str = "CONFIG_DIR";
const PATH_VARIABLE: &str = "CONFIG_PATH";
const FILE_NAME: &str = "config.toml";

/// Where `application` keeps its configuration: `<APPLICATION>_CONFIG_DIR` if
/// it is set, otherwise a dot directory named for the application under the
/// home directory.
pub fn config_dir(application: &Application) -> Result<PathBuf, ConfigError> {
    if let Some(configured) = configured(application, DIRECTORY_VARIABLE) {
        return Ok(configured);
    }

    let home = dirs::home_dir().ok_or(ConfigError::NoHomeDirectory)?;
    Ok(home.join(format!(".{application}")))
}

/// The configuration file `application` loads: `<APPLICATION>_CONFIG_PATH` if
/// it is set, otherwise `config.toml` inside [`config_dir`].
pub fn config_path(application: &Application) -> Result<PathBuf, ConfigError> {
    if let Some(configured) = configured(application, PATH_VARIABLE) {
        return Ok(configured);
    }

    Ok(config_dir(application)?.join(FILE_NAME))
}

fn configured(application: &Application, suffix: &str) -> Option<PathBuf> {
    env::var_os(variable(application, suffix))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn variable(application: &Application, suffix: &str) -> String {
    let prefix: String = application
        .as_str()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();

    format!("{prefix}_{suffix}")
}

#[cfg(test)]
mod tests {
    use std::path::Component;
    use std::path::Path;

    use super::*;

    fn application(name: &str) -> Application {
        Application::new(name).unwrap()
    }

    #[test]
    fn the_directory_is_named_for_the_application() {
        let directory = config_dir(&application("example")).unwrap();
        assert!(directory.ends_with(".example"), "{directory:?}");
    }

    #[test]
    fn the_directory_stays_inside_the_home_directory() {
        let home = dirs::home_dir().unwrap();
        let directory = config_dir(&application("example")).unwrap();

        let relative = directory.strip_prefix(&home).unwrap();
        assert!(
            relative
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
            "{directory:?}"
        );
        assert_eq!(relative.components().count(), 1, "{directory:?}");
    }

    #[test]
    fn the_file_sits_in_the_application_directory() {
        let path = config_path(&application("example")).unwrap();
        assert!(
            path.ends_with(Path::new(".example").join(FILE_NAME)),
            "{path:?}"
        );
    }

    #[test]
    fn the_file_is_toml() {
        let path = config_path(&application("example")).unwrap();
        assert!(path.to_string_lossy().ends_with("config.toml"));
    }

    #[test]
    fn overrides_are_named_for_the_application() {
        assert_eq!(
            variable(&application("example"), PATH_VARIABLE),
            "EXAMPLE_CONFIG_PATH"
        );
        assert_eq!(
            variable(&application("example"), DIRECTORY_VARIABLE),
            "EXAMPLE_CONFIG_DIR"
        );
    }

    #[test]
    fn override_names_are_valid_variable_names() {
        assert_eq!(
            variable(&application("my-app"), PATH_VARIABLE),
            "MY_APP_CONFIG_PATH"
        );
        assert_eq!(
            variable(&application("my_app"), PATH_VARIABLE),
            "MY_APP_CONFIG_PATH"
        );
    }

    #[test]
    fn an_unset_override_falls_through() {
        assert!(
            configured(
                &application("abnegate-config-no-such-application"),
                PATH_VARIABLE
            )
            .is_none()
        );
    }
}
