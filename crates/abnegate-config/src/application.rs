mod error;

use std::fmt;
use std::str::FromStr;

pub use crate::application::error::ApplicationError;

/// The name a default [`Application`] carries.
pub const DEFAULT_APPLICATION: &str = "abnegate";

const SEPARATORS: [char; 2] = ['-', '_'];

/// The name an application keeps its own files under, and derives its
/// configuration directory, environment overrides and keyring service from.
///
/// A name starts with an ASCII letter or digit and continues with ASCII
/// letters, digits, `-` and `_`, so the hidden `.{name}` directory it names is
/// always one plain directory: it can never climb out of the directory it is
/// joined to, and never leaves the files directly inside it.
///
/// ```
/// use abnegate_config::Application;
/// use abnegate_config::DEFAULT_APPLICATION;
///
/// let application = Application::new("example-cli")?;
/// assert_eq!(application.as_str(), "example-cli");
/// assert_eq!(application.directory(), ".example-cli");
///
/// assert!(Application::new("../example").is_err());
/// assert_eq!(Application::default().as_str(), DEFAULT_APPLICATION);
/// # Ok::<(), abnegate_config::ApplicationError>(())
/// ```
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Application {
    name: String,
}

impl Application {
    /// An application called `name`, failing with
    /// [`ApplicationError::Invalid`] when it is not one plain name.
    pub fn new(name: impl Into<String>) -> Result<Self, ApplicationError> {
        let name = name.into();

        if !is_valid(&name) {
            return Err(ApplicationError::Invalid { name });
        }

        Ok(Self { name })
    }

    /// The name, exactly as it was given.
    pub fn as_str(&self) -> &str {
        &self.name
    }

    /// The hidden directory the application keeps its files under, `.{name}`.
    pub fn directory(&self) -> String {
        format!(".{}", self.name)
    }
}

impl Default for Application {
    /// The application called [`DEFAULT_APPLICATION`].
    fn default() -> Self {
        Self {
            name: DEFAULT_APPLICATION.to_string(),
        }
    }
}

impl AsRef<str> for Application {
    fn as_ref(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for Application {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.name)
    }
}

impl FromStr for Application {
    type Err = ApplicationError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::new(name)
    }
}

impl TryFrom<&str> for Application {
    type Error = ApplicationError;

    fn try_from(name: &str) -> Result<Self, Self::Error> {
        Self::new(name)
    }
}

impl TryFrom<String> for Application {
    type Error = ApplicationError;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        Self::new(name)
    }
}

fn is_valid(name: &str) -> bool {
    let mut characters = name.chars();

    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && characters
            .all(|character| character.is_ascii_alphanumeric() || SEPARATORS.contains(&character))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_name_is_accepted() {
        for name in [
            "abnegate",
            "example",
            "example-cli",
            "example_cli",
            "Example2",
            "0x",
            "9lives",
            "A",
        ] {
            let application = Application::new(name).expect(name);

            assert_eq!(application.as_str(), name);
            assert_eq!(application.directory(), format!(".{name}"));
        }
    }

    #[test]
    fn a_name_that_could_escape_its_directory_is_rejected() {
        for name in [
            "",
            ".",
            "..",
            "./example",
            "../example",
            "./../../etc",
            "example/../other",
            "/etc",
            "example/nested",
            "example\\nested",
            ".hidden",
            "-leading",
            "_leading",
            "with space",
            "with.dot",
            "exämple",
            "é",
            "example\0",
        ] {
            assert_eq!(
                Application::new(name),
                Err(ApplicationError::Invalid {
                    name: name.to_string()
                }),
                "{name:?}"
            );
        }
    }

    #[test]
    fn the_default_is_the_default_application() {
        let application = Application::default();

        assert_eq!(application.as_str(), DEFAULT_APPLICATION);
        assert_eq!(application.directory(), ".abnegate");
        assert_eq!(Application::new(DEFAULT_APPLICATION), Ok(application));
    }

    #[test]
    fn every_conversion_validates() {
        assert_eq!(
            "example".parse::<Application>().unwrap(),
            Application::try_from("example").unwrap()
        );
        assert!("../example".parse::<Application>().is_err());
        assert!(Application::try_from("../example".to_string()).is_err());
    }

    #[test]
    fn an_application_displays_as_its_name() {
        let application = Application::new("example-cli").unwrap();

        assert_eq!(application.to_string(), "example-cli");
        assert_eq!(application.as_ref(), "example-cli");
    }
}
