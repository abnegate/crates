use std::collections::BTreeMap;
use std::collections::BTreeSet;

/// The paths whose entries differ between two `git ls-files --stage -z`
/// listings: staged, unstaged, resolved or removed since the first was taken.
pub(super) fn changed(before: &str, after: &str) -> BTreeSet<String> {
    let before = entries(before);
    let after = entries(after);
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .map(|path| path.to_string())
        .collect()
}

/// Each path in a listing and the mode, object and stage of every entry it
/// has: one when merged, up to three while it conflicts.
fn entries(listing: &str) -> BTreeMap<&str, Vec<&str>> {
    let mut entries: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for entry in listing.split('\0').filter(|entry| !entry.is_empty()) {
        if let Some((metadata, path)) = entry.split_once('\t') {
            entries.entry(path).or_default().push(metadata);
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(entries: &[&str]) -> String {
        entries.iter().map(|entry| format!("{entry}\0")).collect()
    }

    fn merged() -> String {
        listing(&[
            "100644 aaaa 0\tREADME.md",
            "100644 bbbb 1\tsrc/value.rs",
            "100644 cccc 2\tsrc/value.rs",
            "100644 dddd 3\tsrc/value.rs",
        ])
    }

    #[test]
    fn an_unchanged_index_changes_nothing() {
        assert!(changed(&merged(), &merged()).is_empty());
    }

    #[test]
    fn a_resolved_staged_added_or_removed_path_is_changed() {
        let after = listing(&[
            "100644 eeee 0\tREADME.md",
            "100644 ffff 0\tsrc/value.rs",
            "100644 9999 0\tnew.rs",
        ]);

        assert_eq!(
            changed(&merged(), &after),
            BTreeSet::from([
                "README.md".to_string(),
                "new.rs".to_string(),
                "src/value.rs".to_string(),
            ])
        );
        assert_eq!(
            changed(&merged(), &listing(&["100644 aaaa 0\tREADME.md"])),
            BTreeSet::from(["src/value.rs".to_string()])
        );
    }
}
