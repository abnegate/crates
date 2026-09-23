use thiserror::Error;

/// Why a name cannot be an [`Application`](super::Application).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ApplicationError {
    #[error(
        "{0:?} is not an application name: use ASCII letters, digits, '_' and '-', starting with a letter or digit"
    )]
    Invalid(String),
}
