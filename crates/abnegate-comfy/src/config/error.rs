use crate::client::Error;
use crate::lora::TrainError;

/// A [`Config`](crate::Config) setting that cannot work as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ConfigError(&'static str);

impl ConfigError {
    pub(crate) const fn new(message: &'static str) -> Self {
        Self(message)
    }

    pub fn message(&self) -> &'static str {
        self.0
    }
}

impl From<ConfigError> for Error {
    fn from(error: ConfigError) -> Self {
        Self::Configuration(error.message())
    }
}

impl From<ConfigError> for TrainError {
    fn from(error: ConfigError) -> Self {
        Self::Configuration(error.message())
    }
}
