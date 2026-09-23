//! Git operations, run by shelling out to `git`.
//!
//! [`GitService`] covers two kinds of repository. A *hardened* one is reached
//! over HTTPS at github.com and cloned and pushed with a credential that never
//! enters the URL, the process table or `.git/config`. Every command against
//! it runs with the host's configuration ignored and the settings that run a
//! hook, a monitor, a helper or a signing program pinned on the command line;
//! and because a run's own git commands can write the repository's
//! configuration where no pin reaches -- a transport rewrite, an HTTP
//! override, an include, a driver, a configured hook -- a repository whose
//! configuration holds anything beyond what git itself writes for a clone is
//! refused before anything runs in it. The check is made before each
//! command, so it cannot stop a run that rewrites the configuration in the
//! instant between the check and the command; a clone every run can write is
//! not one a credential should be sent from. A
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
mod worktree_entry;

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
pub(crate) use crate::git::worktree_entry::WorktreeEntry;
