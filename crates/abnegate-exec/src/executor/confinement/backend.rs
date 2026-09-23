use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;

use super::error::ConfinementError;
use super::mode::ConfinementMode;
use super::seatbelt::DENIED_FILES;

const SEATBELT: &str = "/usr/bin/sandbox-exec";
const BUBBLEWRAP: &str = "/usr/bin/bwrap";

/// The OS mechanism used to confine a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Backend {
    /// macOS `sandbox-exec` with a generated profile
    Seatbelt,
    /// Linux `bwrap` with fresh namespaces
    Bubblewrap,
}

/// The backend this host confines with, if it has one.
#[cfg(target_os = "macos")]
pub const HOST_BACKEND: Option<Backend> = Some(Backend::Seatbelt);

/// The backend this host confines with, if it has one.
#[cfg(target_os = "linux")]
pub const HOST_BACKEND: Option<Backend> = Some(Backend::Bubblewrap);

/// The backend this host confines with, if it has one.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub const HOST_BACKEND: Option<Backend> = None;

impl Backend {
    /// Absolute path of the backend executable.
    pub const fn executable(self) -> &'static str {
        match self {
            Backend::Seatbelt => SEATBELT,
            Backend::Bubblewrap => BUBBLEWRAP,
        }
    }

    /// A file the sandbox must refuse to read, used by the probe.
    pub(super) const fn denied_system_file(self) -> &'static str {
        match self {
            Backend::Seatbelt => DENIED_FILES[0],
            Backend::Bubblewrap => "/etc/hosts",
        }
    }

    /// Whether a [`ConfinementMode::ProcessTree`] job can execute only from
    /// its execute roots.
    ///
    /// Seatbelt filters `process-exec` by path, so a binary dropped into a
    /// writable root stays unrunnable. Bubblewrap has no exec filter: every
    /// file the mount namespace holds is executable, which includes the read
    /// and write roots and the system trees mounted for the loader. A backend
    /// without this cannot prove the tree claim, so it refuses tree jobs and
    /// does not advertise `confinement_process_tree`.
    ///
    /// [`ConfinementMode::ProcessTree`]: super::ConfinementMode::ProcessTree
    pub const fn enforces_execute_roots(self) -> bool {
        match self {
            Backend::Seatbelt => true,
            Backend::Bubblewrap => false,
        }
    }

    /// Whether a [`ConfinementMode::SingleCommand`] job is one process that
    /// can neither fork nor exec.
    ///
    /// Seatbelt filters `process-fork` and `process-exec`, so it is. Bubblewrap
    /// has neither filter: the command may fork, and exec anything the mount
    /// namespace holds, inside the same filesystem and network confinement,
    /// dying with the sandbox through `--die-with-parent` and PID namespace
    /// teardown. A backend without this still runs single-command jobs, but
    /// does not advertise `confinement_single_process`.
    ///
    /// [`ConfinementMode::SingleCommand`]: super::ConfinementMode::SingleCommand
    pub const fn enforces_single_process(self) -> bool {
        match self {
            Backend::Seatbelt => true,
            Backend::Bubblewrap => false,
        }
    }
}

impl Backend {
    /// Refuse a mode whose claim this backend has no mechanism to hold.
    ///
    /// A single-command job's claim on a backend that does not enforce a
    /// single process is the filesystem and network confinement alone, which
    /// every backend holds. A tree's claim is its execute bound, and nothing
    /// can stand in for that.
    pub(super) fn require(self, mode: ConfinementMode) -> Result<(), ConfinementError> {
        match mode {
            ConfinementMode::ProcessTree if !self.enforces_execute_roots() => {
                Err(ConfinementError::Unproven(format!(
                    "{} cannot bound which executables a process tree runs",
                    self.executable()
                )))
            }
            _ => Ok(()),
        }
    }
}

pub(super) fn backend_executable(backend: Backend) -> Result<PathBuf, ConfinementError> {
    usable_backend(Path::new(backend.executable()))
}

fn usable_backend(path: &Path) -> Result<PathBuf, ConfinementError> {
    let unusable = |reason: String| ConfinementError::BackendUnusable {
        path: path.display().to_string(),
        reason,
    };

    let metadata = fs::metadata(path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => unusable("not installed".to_string()),
        io::ErrorKind::PermissionDenied => {
            unusable("its directory is not searchable by this user".to_string())
        }
        _ => unusable(format!("cannot be inspected: {error}")),
    })?;

    if !metadata.is_file() {
        return Err(unusable("not a regular file".to_string()));
    }

    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(unusable(format!(
            "not executable (mode {:04o})",
            metadata.permissions().mode() & 0o7777
        )));
    }

    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unusable_backend_says_which_way_it_is_unusable() {
        let directory = tempfile::tempdir().expect("temporary directory");

        let absent = directory.path().join("sandbox-exec");
        assert_eq!(
            usable_backend(&absent),
            Err(ConfinementError::BackendUnusable {
                path: absent.display().to_string(),
                reason: "not installed".to_string(),
            })
        );

        let not_a_file = directory.path().join("subdirectory");
        fs::create_dir(&not_a_file).expect("directory is created");
        assert_eq!(
            usable_backend(&not_a_file),
            Err(ConfinementError::BackendUnusable {
                path: not_a_file.display().to_string(),
                reason: "not a regular file".to_string(),
            })
        );

        let unreadable = directory.path().join("not-executable");
        fs::write(&unreadable, b"#!/bin/sh\n").expect("file is written");
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644))
            .expect("permissions are set");
        assert_eq!(
            usable_backend(&unreadable),
            Err(ConfinementError::BackendUnusable {
                path: unreadable.display().to_string(),
                reason: "not executable (mode 0644)".to_string(),
            })
        );

        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o755))
            .expect("permissions are set");
        assert_eq!(usable_backend(&unreadable), Ok(unreadable));
    }

    #[test]
    fn test_backend_executables() {
        assert_eq!(Backend::Seatbelt.executable(), SEATBELT);
        assert_eq!(Backend::Bubblewrap.executable(), BUBBLEWRAP);
    }

    #[test]
    fn a_tree_is_refused_where_the_backend_cannot_bound_its_execs() {
        assert!(matches!(
            Backend::Bubblewrap.require(ConfinementMode::ProcessTree),
            Err(ConfinementError::Unproven(_))
        ));
        assert_eq!(
            Backend::Seatbelt.require(ConfinementMode::ProcessTree),
            Ok(())
        );
    }

    #[test]
    fn a_single_command_is_confined_on_every_backend() {
        for backend in [Backend::Seatbelt, Backend::Bubblewrap] {
            assert_eq!(backend.require(ConfinementMode::SingleCommand), Ok(()));
        }
    }
}
