mod error;

use std::fmt;
use std::str::FromStr;

pub use crate::application::error::ApplicationError;

const SEPARATORS: [char; 2] = ['-', '_'];

/// The name an application's configuration directory, environment overrides
/// and keyring service are derived from.
///
/// A name starts with an ASCII letter or digit and continues with ASCII
/// letters, digits, `-` and `_`, so it can never climb out of the home
/// directory or name a hidden path of its own.
///
/// ```
/// use abnegate_config::Application;
///
/// assert_eq!(Application::new("example-cli")?.as_str(), "example-cli");
/// assert!(Application::new("../example").is_err());
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
        for name in ["example", "example-cli", "example_cli", "Example2", "0x"] {
            assert_eq!(Application::new(name).unwrap().as_str(), name);
        }
    }

    #[test]
    fn a_name_that_could_escape_its_directory_is_rejected() {
        for name in [
            "",
            ".",
            "..",
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
    fn every_conversion_validates() {
        assert!("example".parse::<Application>().is_ok());
        assert!("../example".parse::<Application>().is_err());
        assert!(Application::try_from("example").is_ok());
        assert!(Application::try_from("../example".to_string()).is_err());
    }

    #[test]
    fn an_application_displays_as_its_name() {
        let application = Application::new("example-cli").unwrap();

        assert_eq!(application.to_string(), "example-cli");
        assert_eq!(application.as_ref(), "example-cli");
    }
}
