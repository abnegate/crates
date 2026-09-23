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
//! refused before anything runs in it, as is one whose git directory holds a
//! symbolic link anywhere, since git writes through such a link to wherever
//! it points, in a file's place or a directory's, and a checkout whose git
//! directory, or the one it shares, is not the one its own `.git` names or
//! not the clone it was made from: a `.git` that is a link, or a file or
//! worktree record rewritten to name another clone's, would hand every write
//! to that other repository, and a record another clone kept for a worktree
//! that once stood at the same path is as consistent in itself as the
//! clone's own. A repository that borrows objects from another store through
//! `objects/info/alternates` is refused as well, since a push would upload
//! whatever the pushed commit reaches there. A checkout is a
//! [`Checkout`](crate::Checkout), named by its top, where its `.git` stands,
//! and bound to the clone it was made from; a path below the top, or with no
//! `.git` in it at all, is refused with [`GitError::NotACheckoutTop`], since
//! git run there reaches whichever repository encloses it. The operations
//! that keep a base clone in step with its remote, and the ones that add and
//! remove its worktrees, take the clone's top as a path and refuse anything
//! but the clone. The
//! check is made before each command, so it cannot stop a run that rewrites the
//! repository in the instant between the check and the command; a clone
//! every run can write is not one a credential should be sent from. A
//! *managed* one is a local clone the caller owns outright: it is cloned and
//! fetched from the address the caller configured, over a local path, HTTPS
//! or SSH and no other transport, under a timeout and with the caller's own
//! environment. Every worktree of it shares its configuration and hooks, and a
//! run works in one, so it is held to the same check before it is fetched,
//! checked out, reset or given a worktree, and every one of those commands
//! carries the same pins; the local ones also ignore the host's
//! configuration. A fetch also goes ahead only while the clone's `origin` is
//! still exactly the address the caller configured and fetches exactly the
//! refspec git writes for a clone, and it writes only what the refspec on its
//! own command line names, so a run can neither point it at a host of its
//! choosing nor narrow it to leave a commit of its own standing as
//! `origin`'s. A managed fetch leaves off only the pins that would blank the
//! caller's credential helper and proxy: the clone is the caller's own and
//! its configuration is checked against the allowlist immediately before
//! every fetch, so the caller's global helper and proxy stay usable. The
//! hardened commands keep every pin.

mod anchor;
mod authentication;
mod diff_summary;
mod error;
#[cfg(unix)]
mod group;
mod hardening;
mod remote_head;
mod service;
mod worktree_entry;

pub(crate) use crate::git::anchor::Anchor;
pub use crate::git::diff_summary::DiffSummary;
pub use crate::git::error::GitError;
pub use crate::git::error::GitResult;
pub(crate) use crate::git::hardening::CONFIG_LISTING;
pub(crate) use crate::git::hardening::DIFF_PREFIX;
pub(crate) use crate::git::hardening::GITLINK_MODE;
pub(crate) use crate::git::hardening::IGNORE_SUBMODULES;
pub(crate) use crate::git::hardening::LOCATING;
pub(crate) use crate::git::hardening::NO_FETCH_HEAD;
pub(crate) use crate::git::hardening::PINS;
pub(crate) use crate::git::hardening::harden;
pub(crate) use crate::git::hardening::native;
pub(crate) use crate::git::hardening::refused;
pub use crate::git::remote_head::RemoteHead;
pub use crate::git::service::GitService;
#[cfg(test)]
pub(crate) use crate::git::service::fixtures;
pub(crate) use crate::git::worktree_entry::WorktreeEntry;
