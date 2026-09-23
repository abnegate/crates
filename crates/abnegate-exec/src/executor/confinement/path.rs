use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;

use super::error::ConfinementError;

pub(super) fn text(path: &Path) -> Result<&str, ConfinementError> {
    let value = path
        .to_str()
        .ok_or_else(|| ConfinementError::NonUnicodePath(path.display().to_string()))?;
    if !path.is_absolute() {
        return Err(ConfinementError::RelativePath(value.to_string()));
    }
    if value.chars().any(char::is_control) {
        return Err(ConfinementError::ControlCharacterInPath(value.to_string()));
    }
    Ok(value)
}

pub(super) fn canonical(path: &Path) -> Result<PathBuf, ConfinementError> {
    let canonical = fs::canonicalize(path).map_err(|error| ConfinementError::UnusablePath {
        path: path.display().to_string(),
        reason: error.to_string(),
    })?;
    text(&canonical)?;
    Ok(canonical)
}

pub(super) fn canonical_roots(roots: &[PathBuf]) -> Result<Vec<PathBuf>, ConfinementError> {
    let mut canonical_roots: Vec<PathBuf> = Vec::with_capacity(roots.len());
    for root in roots {
        let root = canonical(root)?;
        if !canonical_roots.contains(&root) {
            canonical_roots.push(root);
        }
    }
    Ok(canonical_roots)
}

pub(super) fn executable_file(path: &Path) -> Option<PathBuf> {
    let canonical = fs::canonicalize(path).ok()?;
    let metadata = fs::metadata(&canonical).ok()?;
    (metadata.is_file() && metadata.permissions().mode() & 0o111 != 0).then_some(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_rejects_relative_paths() {
        assert_eq!(
            text(Path::new("relative/path")),
            Err(ConfinementError::RelativePath("relative/path".to_string()))
        );
    }

    #[test]
    fn test_text_rejects_control_characters() {
        assert!(matches!(
            text(Path::new("/tmp/a\nb")),
            Err(ConfinementError::ControlCharacterInPath(_))
        ));
    }
}
