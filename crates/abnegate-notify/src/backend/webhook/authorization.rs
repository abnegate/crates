//! The credential a webhook presents in its `Authorization` header.

use abnegate_secret::SecretValue;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::RequestBuilder;

use crate::error::Error;

/// How a [`Webhook`](crate::Webhook) authenticates, beyond its URL.
///
/// The credential stays a [`SecretValue`], so formatting it prints
/// `[REDACTED]`, and reqwest marks the header it becomes as sensitive.
#[derive(Clone, Debug)]
pub(crate) enum Authorization {
    /// `Authorization: Bearer <token>`.
    Bearer(SecretValue),

    /// `Authorization: Basic <encoded>`, where `encoded` is the base64 of
    /// `user:password` exactly as it goes on the wire, kept so an echoed
    /// header can be concealed too.
    Basic {
        user: SecretValue,
        password: SecretValue,
        encoded: SecretValue,
    },
}

impl Authorization {
    pub(crate) fn basic(user: String, password: SecretValue) -> Self {
        let encoded = SecretValue::new(STANDARD.encode(format!("{user}:{}", password.expose())));
        Self::Basic {
            user: SecretValue::new(user),
            password,
            encoded,
        }
    }

    /// The credential on `request`, or [`Error::Malformed`] for a basic user
    /// name holding a colon, which a receiver would split into the password.
    pub(crate) fn apply(&self, request: RequestBuilder) -> Result<RequestBuilder, Error> {
        match self {
            Self::Bearer(token) => Ok(request.bearer_auth(token.expose())),
            Self::Basic { user, .. } if user.expose().contains(':') => Err(Error::malformed(
                "a basic authentication user name cannot contain ':'",
            )),
            Self::Basic { user, password, .. } => {
                Ok(request.basic_auth(user.expose(), Some(password.expose())))
            }
        }
    }

    /// Every form of this credential that a provider could echo back.
    pub(crate) fn secrets(&self) -> Vec<&SecretValue> {
        match self {
            Self::Bearer(token) => vec![token],
            Self::Basic {
                password, encoded, ..
            } => vec![password, encoded],
        }
    }
}
