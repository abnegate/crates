use std::error::Error;
use std::fmt;

use crate::provider::ProviderError;

/// What went wrong when asking a model.
///
/// Only an [`AiClient`](crate::AiClient) builds one. A variant may gain a
/// field in a minor release, so a pattern outside this crate ends in `..`:
///
/// ```compile_fail,E0638
/// use abnegate_llm::AiError;
///
/// fn wanted(error: &AiError) -> Option<&str> {
///     match error {
///         AiError::NoProvider { wanted } => Some(wanted),
///         _ => None,
///     }
/// }
/// # let _ = wanted;
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum AiError {
    /// This run has no provider. Not a failure, a configuration.
    #[non_exhaustive]
    NoProvider {
        /// What the caller was trying to do, so the message can say what is
        /// being given up.
        wanted: String,
    },
    /// The provider was reached and refused, or could not be reached.
    Provider(ProviderError),
    /// An answer came back but was not the shape that was asked for.
    #[non_exhaustive]
    Shape {
        /// The name of the schema the answer was asked to fit.
        schema: String,
        /// How the answer missed it.
        detail: String,
    },
}

impl fmt::Display for AiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Short, because a run that has no provider hits this at every
            // step that needs one; the advice on how to get one belongs with
            // the first notice, not repeated a dozen times.
            AiError::NoProvider { wanted } => {
                write!(
                    formatter,
                    "{wanted} needs an AI provider; this run has none"
                )
            }
            AiError::Provider(error) => write!(formatter, "the provider failed: {error}"),
            AiError::Shape { schema, detail } => write!(
                formatter,
                "the model's answer did not fit {schema}: {detail}. The answer is discarded \
                 rather than half-read."
            ),
        }
    }
}

impl Error for AiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            AiError::Provider(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ProviderError> for AiError {
    fn from(error: ProviderError) -> Self {
        AiError::Provider(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_error_is_kept_as_the_source() {
        let error = AiError::from(ProviderError::network("refused"));
        assert!(error.to_string().contains("refused"));
        assert!(error.source().is_some());
    }

    #[test]
    fn a_shape_failure_names_the_schema_and_says_the_answer_was_dropped() {
        let error = AiError::Shape {
            schema: "the facts".into(),
            detail: "missing field".into(),
        };
        let message = error.to_string();
        assert!(message.contains("the facts"), "{message}");
        assert!(message.contains("discarded"), "{message}");
        assert!(error.source().is_none());
    }
}
