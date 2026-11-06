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
//! [`ApplicationService`] asks GitHub about the App itself: it registers an
//! App from its manifest with `convert_manifest`, which yields a
//! `RegisteredApplication`; fetches, lists and deletes the App's
//! installations with `fetch_installation`, `list_installations` and
//! [`ApplicationService::delete_installation`], each an `Installation`; and
//! lists the repositories an installation reaches with
//! `list_installation_repositories`, each an `InstalledRepository`. Every
//! request but registering one carries the App's JWT or an installation's
//! token, and none follows a redirect.
//!
//! [`SecretValue`]: abnegate_secret::SecretValue

mod account;
mod account_type;
mod claims;
mod error;
#[cfg(test)]
mod fixtures;
mod github_account;
mod github_installation;
mod id;
mod installation;
mod installation_id;
mod installation_issuer;
mod installation_token;
mod repository_selection;
mod service;
mod token_cache;

pub use crate::pull_request::application::account::Account;
pub use crate::pull_request::application::account_type::AccountType;
pub use crate::pull_request::application::claims::ApplicationClaims;
pub use crate::pull_request::application::error::ApplicationError;
pub use crate::pull_request::application::id::ApplicationId;
pub use crate::pull_request::application::installation::Installation;
pub use crate::pull_request::application::installation_id::InstallationId;
pub use crate::pull_request::application::installation_issuer::InstallationIssuer;
pub use crate::pull_request::application::installation_token::InstallationToken;
pub use crate::pull_request::application::repository_selection::RepositorySelection;
pub use crate::pull_request::application::service::ApplicationService;
pub use crate::pull_request::application::token_cache::TokenCache;

/// The REST API version every GitHub App request is asked under.
const API_VERSION: &str = "2022-11-28";

/// The header GitHub reads the REST API version a request wants from.
const API_VERSION_HEADER: &str = "X-GitHub-Api-Version";
