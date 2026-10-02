//! Walk a source tree for files worth indexing.

use std::path::Path;
use std::path::PathBuf;

/// Directory names skipped while walking, lowercased.
pub const SKIP_DIRECTORIES: &[&str] = &[
    "node_modules",
    "vendor",
    "tmp",
    "log",
    "spec",
    "test",
    "tests",
    "target",
    "dist",
    "build",
    "webkitbuild",
    "__pycache__",
    "deriveddata",
    "pods",
    "generated",
];

/// File extensions collected while walking, lowercased.
pub const EXTENSIONS: &[&str] = &[
    "rb", "js", "ts", "jsx", "tsx", "vue", "go", "graphql", "yml", "yaml", "liquid", "erb", "haml",
    "c", "cc", "cpp", "cxx", "h", "hpp", "hh", "rs", "py", "java", "kt", "php", "swift", "m", "mm",
    "scala", "cs", "syz", "txt", "md", "json",
];

/// Files larger than this are not walked.
pub const LARGEST_FILE: u64 = 1_048_576;

/// Which files a walk collects.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Walk {
    /// Directory names skipped, compared case-insensitively.
    pub skip_directories: Vec<String>,
    /// File extensions collected, compared case-insensitively.
    pub extensions: Vec<String>,
    /// Files larger than this are not collected.
    pub largest_file: u64,
}

impl Default for Walk {
    fn default() -> Self {
        Self::new()
    }
}

impl Walk {
    /// The default skip list, extension list, and file size ceiling.
    pub fn new() -> Self {
        Self {
            skip_directories: SKIP_DIRECTORIES
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            extensions: EXTENSIONS.iter().map(|name| (*name).to_string()).collect(),
            largest_file: LARGEST_FILE,
        }
    }

    /// This walk, skipping `names` instead of the default list.
    pub fn with_skip_directories(
        mut self,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.skip_directories = names.into_iter().map(Into::into).collect();
        self
    }

    /// This walk, collecting `names` instead of the default extensions.
    pub fn with_extensions(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.extensions = names.into_iter().map(Into::into).collect();
        self
    }

    /// This walk, skipping files larger than `bytes`.
    pub fn with_largest_file(mut self, bytes: u64) -> Self {
        self.largest_file = bytes;
        self
    }

    /// Every matching file under `root`.
    pub fn collect(&self, root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        self.visit(root, &mut files);
        files
    }

    fn visit(&self, directory: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|item| item.to_str())
                .unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if self.skipped_directory(name) {
                    continue;
                }
                self.visit(&path, out);
                continue;
            }
            let Some(ext) = path.extension().and_then(|item| item.to_str()) else {
                continue;
            };
            if !self.known_extension(ext) {
                continue;
            }
            if std::fs::metadata(&path)
                .ok()
                .is_some_and(|meta| meta.len() > self.largest_file)
            {
                continue;
            }
            out.push(path);
        }
    }

    fn skipped_directory(&self, name: &str) -> bool {
        self.skip_directories
            .iter()
            .any(|skipped| skipped.eq_ignore_ascii_case(name))
    }

    fn known_extension(&self, ext: &str) -> bool {
        self.extensions
            .iter()
            .any(|known| known.eq_ignore_ascii_case(ext))
    }
}

/// Every matching file under `root` using [`Walk::new`].
pub fn walk_source(root: &Path) -> Vec<PathBuf> {
    Walk::new().collect(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_skips_dotfiles_and_vendor_trees() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("lib")).unwrap();
        std::fs::create_dir_all(root.path().join("node_modules")).unwrap();
        std::fs::write(root.path().join("lib").join("api.rb"), "class API\nend\n").unwrap();
        std::fs::write(
            root.path().join("node_modules").join("x.js"),
            "module.exports=1\n",
        )
        .unwrap();
        std::fs::write(root.path().join(".hidden.rs"), "fn x() {}\n").unwrap();
        let files = walk_source(root.path());
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("api.rb"));
    }

    #[test]
    fn walk_skips_files_above_the_ceiling() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("tiny.rs"), "fn x(){}\n").unwrap();
        std::fs::write(root.path().join("huge.rs"), "fn huge() { todo!() }\n").unwrap();
        let files = Walk::new().with_largest_file(10).collect(root.path());
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("tiny.rs"));
    }
}
