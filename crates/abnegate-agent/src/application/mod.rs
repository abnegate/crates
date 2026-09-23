mod error;

pub use error::ApplicationError;

use std::fmt;
use std::str::FromStr;

/// The application name a default [`Application`] carries.
pub const DEFAULT_APPLICATION: &str = "abnegate";

/// The name an application keeps its own files under, as a hidden
/// `.{name}` directory: job logs inside a working tree, saved sessions inside
/// the home directory.
///
/// Only ASCII letters, digits, `_` and `-` are allowed, starting with a letter
/// or digit, so the directory it names is always one plain name. A free-form
/// string let `./x` become `../x` and step out of the checkout, and let an
/// empty one put the files straight into the tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Application(String);

impl Application {
    pub fn new(name: impl Into<String>) -> Result<Self, ApplicationError> {
        let name = name.into();
        let mut characters = name.chars();
        let leading = characters
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric());
        let rest = characters
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'));
        if leading && rest {
            Ok(Self(name))
        } else {
            Err(ApplicationError::Invalid(name))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The hidden directory name, `.{name}`.
    pub fn directory(&self) -> String {
        format!(".{}", self.0)
    }
}

impl Default for Application {
    fn default() -> Self {
        Self(DEFAULT_APPLICATION.to_string())
    }
}

impl fmt::Display for Application {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for Application {
    fn as_ref(&self) -> &str {
        &self.0
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_name_is_an_application() {
        for name in ["abnegate", "zone", "my-app", "app_2", "9lives", "A"] {
            let application = Application::new(name).expect(name);
            assert_eq!(application.as_str(), name);
            assert_eq!(application.directory(), format!(".{name}"));
        }
    }

    /// Each of these made `.{name}` something other than one hidden
    /// directory: `./x` is `../x`, and an empty name is the tree itself.
    #[test]
    fn a_name_that_is_not_one_plain_directory_is_refused() {
        for name in [
            "", "./x", "../x", ".", "..", "a/b", "/abs", "-x", "_x", ".x", "a b", "a\\b", "é",
            "a\0b",
        ] {
            assert_eq!(
                Application::new(name),
                Err(ApplicationError::Invalid(name.to_string())),
                "{name:?}"
            );
        }
    }

    #[test]
    fn the_default_is_the_default_application() {
        assert_eq!(Application::default().as_str(), DEFAULT_APPLICATION);
        assert_eq!(
            "zone".parse::<Application>().unwrap(),
            Application::try_from("zone").unwrap()
        );
    }
}
