use std::env;
use std::path::PathBuf;

use crate::application::Application;
use crate::error::Error;

const DIRECTORY_VARIABLE: &str = "CONFIG_DIRECTORY";
const PATH_VARIABLE: &str = "CONFIG_PATH";
const FILE_NAME: &str = "config.toml";

/// Where `application` keeps its configuration: `<APPLICATION>_CONFIG_DIRECTORY`
/// if it is set, otherwise the application's
/// [`directory`](Application::directory) under the home directory.
///
/// `<APPLICATION>` is the application's name in upper case with every `-`
/// replaced by `_`, so `example-cli` reads `EXAMPLE_CLI_CONFIG_DIRECTORY`.
pub fn directory(application: &Application) -> Result<PathBuf, Error> {
    if let Some(configured) = configured(application, DIRECTORY_VARIABLE) {
        return Ok(configured);
    }

    let home = dirs::home_dir().ok_or(Error::NoHomeDirectory)?;
    Ok(home.join(application.directory()))
}

/// The configuration file `application` loads: `<APPLICATION>_CONFIG_PATH` if
/// it is set, otherwise `config.toml` inside [`directory`].
pub fn path(application: &Application) -> Result<PathBuf, Error> {
    if let Some(configured) = configured(application, PATH_VARIABLE) {
        return Ok(configured);
    }

    Ok(directory(application)?.join(FILE_NAME))
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
    use std::process::Command;

    use super::*;

    const CHILD: &str = "ABNEGATE_CONFIG_TEST_CHILD";

    fn application(name: &str) -> Application {
        Application::new(name).unwrap()
    }

    #[test]
    fn the_directory_is_named_for_the_application() {
        let resolved = directory(&application("example")).unwrap();
        assert!(resolved.ends_with(".example"), "{resolved:?}");
    }

    #[test]
    fn the_directory_stays_inside_the_home_directory() {
        let home = dirs::home_dir().unwrap();
        let resolved = directory(&application("example")).unwrap();

        let relative = resolved.strip_prefix(&home).unwrap();
        assert!(
            relative
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
            "{resolved:?}"
        );
        assert_eq!(relative.components().count(), 1, "{resolved:?}");
    }

    #[test]
    fn the_file_sits_in_the_application_directory() {
        let resolved = path(&application("example")).unwrap();
        assert!(
            resolved.ends_with(Path::new(".example").join(FILE_NAME)),
            "{resolved:?}"
        );
    }

    #[test]
    fn the_file_is_toml() {
        let resolved = path(&application("example")).unwrap();
        assert!(resolved.to_string_lossy().ends_with("config.toml"));
    }

    #[test]
    fn overrides_are_named_for_the_application() {
        assert_eq!(
            variable(&application("example"), PATH_VARIABLE),
            "EXAMPLE_CONFIG_PATH"
        );
        assert_eq!(
            variable(&application("example"), DIRECTORY_VARIABLE),
            "EXAMPLE_CONFIG_DIRECTORY"
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

    #[test]
    fn the_directory_override_moves_the_directory_and_the_file() {
        const NAME: &str = "path::tests::the_directory_override_moves_the_directory_and_the_file";
        const OVERRIDE: &str = "/srv/example-cli";

        let application = application("example-cli");
        if env::var(CHILD).as_deref() == Ok(NAME) {
            assert_eq!(directory(&application).unwrap(), Path::new(OVERRIDE));
            assert_eq!(
                path(&application).unwrap(),
                Path::new(OVERRIDE).join(FILE_NAME)
            );
            return;
        }

        assert_child_passes(
            NAME,
            Command::new(env::current_exe().expect("the test binary"))
                .env("EXAMPLE_CLI_CONFIG_DIRECTORY", OVERRIDE)
                .env_remove("EXAMPLE_CLI_CONFIG_PATH"),
        );
    }

    #[test]
    fn the_retired_directory_variable_is_ignored() {
        const NAME: &str = "path::tests::the_retired_directory_variable_is_ignored";
        const RETIRED: &str = "/srv/example-cli";

        let application = application("example-cli");
        if env::var(CHILD).as_deref() == Ok(NAME) {
            let default = dirs::home_dir().unwrap().join(application.directory());
            assert_eq!(directory(&application).unwrap(), default);
            assert_eq!(path(&application).unwrap(), default.join(FILE_NAME));
            return;
        }

        assert_child_passes(
            NAME,
            Command::new(env::current_exe().expect("the test binary"))
                .env("EXAMPLE_CLI_CONFIG_DIR", RETIRED)
                .env_remove("EXAMPLE_CLI_CONFIG_DIRECTORY")
                .env_remove("EXAMPLE_CLI_CONFIG_PATH"),
        );
    }

    fn assert_child_passes(name: &str, command: &mut Command) {
        let output = command
            .args(["--exact", name, "--nocapture"])
            .env(CHILD, name)
            .output()
            .expect("the child runs");
        let stdout = String::from_utf8_lossy(&output.stdout);

        assert!(
            output.status.success(),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains("1 passed"),
            "the child ran no test, so it proved nothing\n{stdout}"
        );
    }
}
