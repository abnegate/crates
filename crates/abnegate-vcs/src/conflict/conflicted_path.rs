use crate::conflict::ConflictError;
use crate::conflict::ConflictResult;
use std::path::Path;
use std::path::PathBuf;

/// A repository-relative path git reported as unmerged.
///
/// Slash separated, never absolute, never leaving the repository, never an
/// option and never a control character. Globs and pathspec magic need no
/// refusing: every git command here takes pathspecs literally.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConflictedPath(String);

impl ConflictedPath {
    pub fn parse(value: &str) -> ConflictResult<Self> {
        let refused = value.is_empty()
            || value.starts_with('/')
            || value.starts_with('-')
            || value.contains('\\')
            || value.contains("//")
            || value.chars().any(char::is_control)
            || value
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..");

        if refused {
            return Err(ConflictError::UnsafePath(value.to_string()));
        }
        Ok(Self(value.to_string()))
    }

    /// The path as git reported it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ConflictedPath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Resolve one repository-relative path to a real file inside `checkout`.
///
/// Every component is inspected: a symlink anywhere along the way, a final
/// component that is not a regular file, or a canonical path that leaves the
/// checkout all mean the path cannot be repaired safely.
pub fn resolve(checkout: &Path, path: &ConflictedPath) -> ConflictResult<PathBuf> {
    let root = checkout
        .canonicalize()
        .map_err(|_| ConflictError::UnsafePath(path.to_string()))?;

    let mut target = root.clone();
    let components: Vec<&str> = path.as_str().split('/').collect();
    for (index, component) in components.iter().enumerate() {
        target.push(component);
        let details = std::fs::symlink_metadata(&target)
            .map_err(|_| ConflictError::UnsafePath(path.to_string()))?;
        let last = index + 1 == components.len();
        let acceptable = match last {
            true => details.is_file(),
            false => details.is_dir(),
        };
        if details.file_type().is_symlink() || !acceptable {
            return Err(ConflictError::UnsafePath(path.to_string()));
        }
    }

    let canonical = target
        .canonicalize()
        .map_err(|_| ConflictError::UnsafePath(path.to_string()))?;
    if canonical != target || !canonical.starts_with(&root) {
        return Err(ConflictError::UnsafePath(path.to_string()));
    }

    Ok(canonical)
}

/// Resolve every conflicted path, refusing the whole set if any one is unsafe.
pub fn validate(checkout: &Path, files: &[ConflictedPath]) -> ConflictResult<Vec<PathBuf>> {
    if files.is_empty() {
        return Err(ConflictError::NoConflict);
    }
    files.iter().map(|path| resolve(checkout, path)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn a_conflicted_path_must_stay_inside_the_repository() {
        for accepted in ["src/main.rs", "src/[ab] *.rs", ":(top)x", "docs/a (1).md"] {
            assert_eq!(ConflictedPath::parse(accepted).unwrap().as_str(), accepted);
        }
        for invalid in [
            "",
            "/etc/passwd",
            "../outside",
            "src/../../outside",
            "src/./main.rs",
            "src\\main.rs",
            "src/ma\nin.rs",
            "src/\u{7f}",
            "-oops",
        ] {
            assert!(
                ConflictedPath::parse(invalid).is_err(),
                "{invalid:?} must not be accepted as a conflicted path"
            );
        }
    }

    #[test]
    fn an_empty_conflicted_set_is_not_a_conflict() {
        let root = TempDir::new().unwrap();
        assert!(matches!(
            validate(root.path(), &[]),
            Err(ConflictError::NoConflict)
        ));
    }

    #[test]
    fn a_symlinked_conflicted_path_is_refused() {
        let root = TempDir::new().unwrap();
        std::fs::write(root.path().join("real.txt"), "content").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(root.path().join("real.txt"), root.path().join("link.txt"))
            .unwrap();

        let path = ConflictedPath::parse("link.txt").unwrap();
        assert!(
            matches!(
                resolve(root.path(), &path),
                Err(ConflictError::UnsafePath(_))
            ),
            "a symlink can point outside the checkout and must never be repaired"
        );
    }

    #[test]
    fn a_directory_is_not_a_conflicted_file() {
        let root = TempDir::new().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        let path = ConflictedPath::parse("src").unwrap();
        assert!(matches!(
            resolve(root.path(), &path),
            Err(ConflictError::UnsafePath(_))
        ));
    }

    #[test]
    fn a_validated_set_resolves_every_member() {
        let root = TempDir::new().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.path().join("README.md"), "docs").unwrap();

        let files = vec![
            ConflictedPath::parse("src/main.rs").unwrap(),
            ConflictedPath::parse("README.md").unwrap(),
        ];
        let resolved = validate(root.path(), &files).unwrap();
        assert_eq!(resolved.len(), 2);
        assert!(resolved.iter().all(|path| path.is_file()));
    }

    #[test]
    fn one_unsafe_member_refuses_the_whole_set() {
        let root = TempDir::new().unwrap();
        std::fs::write(root.path().join("present.txt"), "here").unwrap();

        let files = vec![
            ConflictedPath::parse("present.txt").unwrap(),
            ConflictedPath::parse("absent.txt").unwrap(),
        ];
        assert!(
            validate(root.path(), &files).is_err(),
            "a set is only as safe as its least safe member"
        );
    }
}
