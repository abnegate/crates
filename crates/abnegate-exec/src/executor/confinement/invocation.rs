use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

/// A backend executable and the argument vector that runs a command inside it.
///
/// `Debug` prints the names in `environment` and never their values.
#[derive(Clone, PartialEq, Eq)]
pub struct Invocation {
    /// The backend executable
    pub program: PathBuf,
    /// Its arguments, ending with the command and the command's arguments
    pub arguments: Vec<String>,
    /// The complete environment of the backend process, which every backend
    /// hands on to the command unchanged. Spawn the backend with exactly this
    /// environment and nothing inherited; values never appear in `arguments`,
    /// where any user on the host could read them.
    pub environment: BTreeMap<String, String>,
}

impl fmt::Debug for Invocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Invocation")
            .field("program", &self.program)
            .field("arguments", &self.arguments)
            .field("environment", &self.environment.keys())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_names_the_environment_without_its_values() {
        let invocation = Invocation {
            program: PathBuf::from("/usr/bin/sandbox-exec"),
            arguments: vec!["--".to_string()],
            environment: BTreeMap::from([("APP_MASTER_KEY".to_string(), "hunter2".to_string())]),
        };

        let debug = format!("{invocation:?}");

        assert!(debug.contains("APP_MASTER_KEY"), "{debug}");
        assert!(!debug.contains("hunter2"), "{debug}");
    }
}
