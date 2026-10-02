//! Path helpers for an indexed tree.

use std::path::Path;

/// `path` relative to `root`, using `/`.
pub fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Canonical `root`, using `/`. Falls back to the given path when canonicalize fails.
pub fn root_key(root: &Path) -> String {
    root.canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn relative_to_strips_the_root() {
        let root = PathBuf::from("/src");
        let path = PathBuf::from("/src/lib/api.rb");
        assert_eq!(relative_to(&root, &path), "lib/api.rb");
    }
}
