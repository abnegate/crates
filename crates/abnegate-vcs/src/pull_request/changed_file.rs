use crate::pull_request::FileStatus;
use serde::Deserialize;

/// One file a pull request changes, and by how much.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct ChangedFile {
    /// Its path in the repository, after any rename.
    pub filename: String,
    /// What the pull request does to it, or [`FileStatus::Unknown`] when
    /// GitHub does not say.
    #[serde(default)]
    pub status: FileStatus,
    /// Lines it adds.
    pub additions: u32,
    /// Lines it removes.
    pub deletions: u32,
    /// Its unified diff hunks, or `None` when GitHub sends none, as it does
    /// for a binary file or a diff too large to show.
    #[serde(default)]
    pub patch: Option<String>,
    /// Its path before the pull request renamed it, or `None` when it was
    /// not renamed.
    #[serde(default)]
    pub previous_filename: Option<String>,
}

impl ChangedFile {
    /// The file at `filename`, which the pull request changes as `status`
    /// says, adding `additions` lines and removing `deletions`.
    ///
    /// It has no diff and was not renamed, until the `with_*` methods say
    /// otherwise.
    pub fn new(
        filename: impl Into<String>,
        status: FileStatus,
        additions: u32,
        deletions: u32,
    ) -> Self {
        Self {
            filename: filename.into(),
            status,
            additions,
            deletions,
            patch: None,
            previous_filename: None,
        }
    }

    /// Its unified diff hunks.
    #[must_use]
    pub fn with_patch(mut self, patch: impl Into<String>) -> Self {
        self.patch = Some(patch.into());
        self
    }

    /// Its path before the pull request renamed it.
    #[must_use]
    pub fn with_previous_filename(mut self, previous_filename: impl Into<String>) -> Self {
        self.previous_filename = Some(previous_filename.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_file_built_from_what_names_it_has_no_diff_and_no_rename() {
        let built = ChangedFile::new("src/basket.rs", FileStatus::Modified, 3, 1);

        assert_eq!(built.filename, "src/basket.rs");
        assert_eq!(built.status, FileStatus::Modified);
        assert_eq!(built.additions, 3);
        assert_eq!(built.deletions, 1);
        assert_eq!(built.patch, None);
        assert_eq!(built.previous_filename, None);
    }

    #[test]
    fn each_builder_sets_its_own_field_and_no_other() {
        let base = ChangedFile::new("src/basket.rs", FileStatus::Renamed, 1, 1);

        let built = base
            .clone()
            .with_patch("@@ -1 +1 @@\n-cart\n+basket")
            .with_previous_filename("src/cart.rs");

        assert_eq!(
            built,
            ChangedFile {
                patch: Some("@@ -1 +1 @@\n-cart\n+basket".to_string()),
                previous_filename: Some("src/cart.rs".to_string()),
                ..base
            }
        );
    }
}
