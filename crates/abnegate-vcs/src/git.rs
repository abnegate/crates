//! Git operations, run by shelling out to `git`.
//!
//! [`GitService`] covers two kinds of repository. A *hardened* one is reached
//! over HTTPS at github.com and cloned and pushed with a credential that never
//! enters the URL, the process table or `.git/config`. Every command against
//! it runs with the host's configuration ignored and the settings that run a
//! hook, a monitor, a helper or a signing program pinned on the command line;
//! and because a run's own git commands can write the repository's
//! configuration where no pin reaches -- a transport rewrite, an HTTP
//! override, an include, a filter, diff or merge driver -- a repository whose
//! configuration holds any of those is refused before anything runs in it. A
//! *managed* one is a local clone the caller owns outright: it is cloned,
//! fetched, reset and given worktrees from whatever address the caller
//! configured, including a local path, under a timeout but with the caller's
//! own environment.

mod authentication;
mod diff_summary;
mod error;
#[cfg(unix)]
mod group;
mod hardening;
mod remote_head;
mod service;

pub use crate::git::diff_summary::DiffSummary;
pub use crate::git::error::GitError;
pub use crate::git::error::GitResult;
pub(crate) use crate::git::hardening::CONFIG_LISTING;
pub(crate) use crate::git::hardening::harden;
pub(crate) use crate::git::hardening::refused;
pub use crate::git::remote_head::RemoteHead;
pub use crate::git::service::GitService;
#[cfg(test)]
pub(crate) use crate::git::service::fixtures;
