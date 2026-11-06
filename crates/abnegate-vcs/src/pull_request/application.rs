//! Authenticating to GitHub as a GitHub App installation.
//!
//! Only the `github-app` feature builds this module. Authenticating is two
//! steps: a JWT signed with the App's private key, from
//! [`ApplicationClaims::sign`], proves the caller is the App, and
//! [`InstallationIssuer::issue`] trades that JWT for an [`InstallationToken`]
//! scoped to one installation and good for an hour.
//! [`TokenCache`] holds each installation's token until it comes within
//! [`InstallationToken::SAFETY_MARGIN`] of expiring. The JWT and the token are
//! both bearer credentials, so both are [`SecretValue`]s from the moment they
//! exist, and no [`ApplicationError`] carries either, the private key, or the
//! URL a request was sent to.
//!
//! [`SecretValue`]: abnegate_secret::SecretValue

mod claims;
mod error;
#[cfg(test)]
mod fixtures;
mod github_installed_repository;
mod id;
mod installation_id;
mod installation_issuer;
mod installation_token;
mod installed_repository;
mod token_cache;

pub use crate::pull_request::application::claims::ApplicationClaims;
pub use crate::pull_request::application::error::ApplicationError;
pub use crate::pull_request::application::id::ApplicationId;
pub use crate::pull_request::application::installation_id::InstallationId;
pub use crate::pull_request::application::installation_issuer::InstallationIssuer;
pub use crate::pull_request::application::installation_token::InstallationToken;
pub use crate::pull_request::application::installed_repository::InstalledRepository;
pub use crate::pull_request::application::token_cache::TokenCache;
