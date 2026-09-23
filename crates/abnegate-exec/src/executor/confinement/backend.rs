use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;

use super::error::ConfinementError;
use super::seatbelt::DENIED_FILES;

const SEATBELT: &str = "/usr/bin/sandbox-exec";
const BUBBLEWRAP: &str = "/usr/bin/bwrap";

/// The OS mechanism used to confine a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Backend {
    Seatbelt,
    Bubblewrap,
}

#[cfg(target_os = "macos")]
pub const HOST_BACKEND: Option<Backend> = Some(Backend::Seatbelt);

#[cfg(target_os = "linux")]
pub const HOST_BACKEND: Option<Backend> = Some(Backend::Bubblewrap);

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

    /// Whether the backend can refuse an exec of a file the tree can otherwise
    /// see.
    ///
    /// Seatbelt filters `process-exec` by path, so a binary dropped into a
    /// writable root stays unrunnable. Bubblewrap has no exec filter: its bound
    /// is the mount namespace, where an executable outside every bind does not
    /// exist at all. Both bound the executable set; only seatbelt can be asked
    /// to prove it against a file that is present.
    pub const fn enforces_execute_roots(self) -> bool {
        match self {
            Backend::Seatbelt => true,
            Backend::Bubblewrap => false,
        }
    }

    /// Whether the backend can refuse the command a second process at all.
    ///
    /// Seatbelt filters `process-fork`, so single-command mode really is one
    /// process. Bubblewrap has no such primitive: it bounds a sandbox by its
    /// namespaces, so a fork succeeds and the child lands inside the same mount,
    /// network and PID namespaces. Containment is identical either way -- the
    /// child sees the same filesystem, reaches no network, and dies with the
    /// sandbox through `--die-with-parent` and PID namespace teardown -- but
    /// only seatbelt can be asked to prevent the second process existing.
    pub const fn enforces_single_process(self) -> bool {
        match self {
            Backend::Seatbelt => true,
            Backend::Bubblewrap => false,
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

    // Reporting every one of these as "not installed" sends an operator to
    // reinstall a backend that is already on disk. A confined job cannot run
    // without one, so the message is the whole of the remedy.
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

        // The case the old message got wrong: the backend is installed, so
        // "not installed" sends the operator to reinstall what is already here.
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
}
