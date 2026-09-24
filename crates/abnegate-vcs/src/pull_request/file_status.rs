use serde::Deserialize;
use std::fmt;

/// What a pull request does to one file, as GitHub names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum FileStatus {
    /// The file is new.
    Added,
    /// The file is deleted.
    Removed,
    /// The file's content changed.
    Modified,
    /// The file moved, and may also have changed.
    Renamed,
    /// The file is a copy of another.
    Copied,
    /// The file changed in a way none of the others names, such as its type.
    Changed,
    /// The file is listed but did not change.
    Unchanged,
    /// GitHub gave no status, or one this crate does not know yet.
    #[default]
    #[serde(other)]
    Unknown,
}

impl FileStatus {
    /// The status as GitHub spells it, or `unknown`.
    pub fn as_str(self) -> &'static str {
        match self {
            FileStatus::Added => "added",
            FileStatus::Removed => "removed",
            FileStatus::Modified => "modified",
            FileStatus::Renamed => "renamed",
            FileStatus::Copied => "copied",
            FileStatus::Changed => "changed",
            FileStatus::Unchanged => "unchanged",
            FileStatus::Unknown => "unknown",
        }
    }
}

impl fmt::Display for FileStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::ChangedFile;
    use crate::pull_request::MergeableState;
    use serde_json::json;

    #[test]
    fn a_file_status_reads_and_prints_as_github_spells_it() {
        for (status, spelled) in [
            (FileStatus::Added, "added"),
            (FileStatus::Removed, "removed"),
            (FileStatus::Modified, "modified"),
            (FileStatus::Renamed, "renamed"),
            (FileStatus::Copied, "copied"),
            (FileStatus::Changed, "changed"),
            (FileStatus::Unchanged, "unchanged"),
            (FileStatus::Unknown, "unknown"),
        ] {
            assert_eq!(
                serde_json::from_value::<FileStatus>(json!(spelled)).unwrap(),
                status
            );
            assert_eq!(status.as_str(), spelled);
            assert_eq!(status.to_string(), spelled);
        }
    }

    #[test]
    fn a_mergeable_state_or_file_status_github_has_not_named_yet_is_unknown() {
        assert_eq!(
            serde_json::from_value::<MergeableState>(json!("queued_for_merge")).unwrap(),
            MergeableState::Unknown
        );
        assert_eq!(MergeableState::default(), MergeableState::Unknown);

        assert_eq!(
            serde_json::from_value::<FileStatus>(json!("type_changed")).unwrap(),
            FileStatus::Unknown
        );
        assert_eq!(FileStatus::default(), FileStatus::Unknown);

        let unnamed: ChangedFile = serde_json::from_value(json!({
            "filename": "src/cart.ts",
            "additions": 3,
            "deletions": 1,
        }))
        .unwrap();
        assert_eq!(
            unnamed,
            ChangedFile {
                filename: "src/cart.ts".to_string(),
                status: FileStatus::Unknown,
                additions: 3,
                deletions: 1,
            },
            "a file GitHub gives no status for reads as unknown rather than failing the listing"
        );

        let renamed: ChangedFile = serde_json::from_value(json!({
            "filename": "src/basket.ts",
            "status": "renamed",
            "additions": 0,
            "deletions": 0,
        }))
        .unwrap();
        assert_eq!(renamed.status, FileStatus::Renamed);
    }
}
