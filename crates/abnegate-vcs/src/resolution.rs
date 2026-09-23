//! Judging whether a repaired file actually resolved its conflict.
//!
//! An agent handed a conflicted file has an obvious cheap escape: delete one
//! side. The file compiles, the markers are gone, and a branch's work has been
//! silently thrown away — which is strictly worse than leaving the conflict for
//! a person. This reads the conflicted original alongside the repaired text and
//! refuses a repair that kept nothing distinctive from one of the two sides.
//!
//! The bound is deliberately at the file level rather than the hunk level. A
//! genuine merge often does take one side of a single hunk, and rejecting that
//! would refuse most correct repairs; what it can never do is come out the other
//! end carrying no trace of a branch that contributed distinct lines.

mod conflict_hunk;
mod conflict_side;
mod region;
mod verdict;

use crate::conflict::has_markers;
use crate::conflict::marker::Marker;
pub use crate::resolution::conflict_hunk::ConflictHunk;
pub use crate::resolution::conflict_side::ConflictSide;
use crate::resolution::region::Region;
pub use crate::resolution::verdict::ResolutionVerdict;
use std::collections::BTreeSet;
use std::collections::HashSet;

/// Split a conflicted file into its hunks.
///
/// A diff3-style hunk carries the merge base between `|||||||` and `=======`;
/// those lines belong to neither side and are dropped, because a line both
/// branches inherited is no evidence that either branch survived.
pub fn hunks(conflicted: &str) -> Vec<ConflictHunk> {
    let mut hunks = Vec::new();
    let mut current = ConflictHunk::default();
    let mut region = Region::Outside;

    for line in conflicted.lines() {
        match (Marker::parse(line), region) {
            (Some(Marker::Ours), _) => {
                current = ConflictHunk::default();
                region = Region::Ours;
            }
            (Some(Marker::Base), Region::Ours) => region = Region::Base,
            (Some(Marker::Split), Region::Ours | Region::Base) => region = Region::Theirs,
            (Some(Marker::Theirs), Region::Theirs) => {
                hunks.push(std::mem::take(&mut current));
                region = Region::Outside;
            }
            (_, Region::Ours) => current.ours.push(line.to_string()),
            (_, Region::Theirs) => current.theirs.push(line.to_string()),
            (_, Region::Base | Region::Outside) => {}
        }
    }

    hunks
}

fn significant(lines: &[String]) -> Vec<&str> {
    lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect()
}

/// Lines one side contributed that the other side did not.
fn distinctive<'a>(side: &'a [String], other: &[String]) -> BTreeSet<&'a str> {
    let shared: HashSet<&str> = significant(other).into_iter().collect();
    significant(side)
        .into_iter()
        .filter(|line| !shared.contains(line))
        .collect()
}

fn survives(present: &HashSet<&str>, lines: &BTreeSet<&str>) -> bool {
    lines.iter().any(|line| present.contains(line))
}

/// Judge a repaired file against the conflicted file it was produced from.
pub fn judge(conflicted: &str, resolved: &str) -> ResolutionVerdict {
    let hunks = hunks(conflicted);
    if hunks.is_empty() {
        return ResolutionVerdict::NoConflict;
    }

    if has_markers(resolved) {
        return ResolutionVerdict::MarkersRemain;
    }

    if resolved.trim().is_empty() && !conflicted.trim().is_empty() {
        return ResolutionVerdict::Emptied;
    }

    let mut ours: BTreeSet<&str> = BTreeSet::new();
    let mut theirs: BTreeSet<&str> = BTreeSet::new();
    for hunk in &hunks {
        ours.extend(distinctive(&hunk.ours, &hunk.theirs));
        theirs.extend(distinctive(&hunk.theirs, &hunk.ours));
    }

    let present: HashSet<&str> = resolved.lines().map(str::trim).collect();
    if !ours.is_empty() && !survives(&present, &ours) {
        return ResolutionVerdict::Discarded(ConflictSide::Ours);
    }
    if !theirs.is_empty() && !survives(&present, &theirs) {
        return ResolutionVerdict::Discarded(ConflictSide::Theirs);
    }

    ResolutionVerdict::Resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFLICTED: &str = "\
fn greet() {
<<<<<<< HEAD
    println!(\"hello from ours\");
    let ours = 1;
=======
    println!(\"hello from theirs\");
    let theirs = 2;
>>>>>>> feature
}
";

    #[test]
    fn a_conflicted_file_splits_into_its_two_sides() {
        let parsed = hunks(CONFLICTED);
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed[0].ours,
            vec![
                "    println!(\"hello from ours\");".to_string(),
                "    let ours = 1;".to_string()
            ]
        );
        assert_eq!(
            parsed[0].theirs,
            vec![
                "    println!(\"hello from theirs\");".to_string(),
                "    let theirs = 2;".to_string()
            ]
        );
    }

    #[test]
    fn a_diff3_hunk_drops_the_merge_base_from_both_sides() {
        let conflicted = "\
<<<<<<< HEAD
ours
||||||| base
inherited
=======
theirs
>>>>>>> feature
";
        let parsed = hunks(conflicted);
        assert_eq!(parsed[0].ours, vec!["ours".to_string()]);
        assert_eq!(parsed[0].theirs, vec!["theirs".to_string()]);
    }

    #[test]
    fn a_file_with_no_hunks_is_nothing_to_judge() {
        assert_eq!(
            judge("plain text\n", "plain text\n"),
            ResolutionVerdict::NoConflict
        );
        assert!(hunks("plain text\n").is_empty());
    }

    #[test]
    fn keeping_both_sides_is_a_resolution() {
        let resolved = "\
fn greet() {
    println!(\"hello from ours\");
    println!(\"hello from theirs\");
    let ours = 1;
    let theirs = 2;
}
";
        assert_eq!(judge(CONFLICTED, resolved), ResolutionVerdict::Resolved);
    }

    #[test]
    fn taking_only_our_side_is_caught_as_discarding_theirs() {
        let resolved = "\
fn greet() {
    println!(\"hello from ours\");
    let ours = 1;
}
";
        assert_eq!(
            judge(CONFLICTED, resolved),
            ResolutionVerdict::Discarded(ConflictSide::Theirs),
            "resolving by deleting a branch's work is worse than not resolving"
        );
    }

    #[test]
    fn taking_only_their_side_is_caught_as_discarding_ours() {
        let resolved = "\
fn greet() {
    println!(\"hello from theirs\");
    let theirs = 2;
}
";
        assert_eq!(
            judge(CONFLICTED, resolved),
            ResolutionVerdict::Discarded(ConflictSide::Ours)
        );
    }

    #[test]
    fn leftover_markers_are_not_a_resolution() {
        assert_eq!(
            judge(CONFLICTED, CONFLICTED),
            ResolutionVerdict::MarkersRemain
        );
    }

    #[test]
    fn emptying_the_file_is_not_a_resolution() {
        assert_eq!(judge(CONFLICTED, "   \n"), ResolutionVerdict::Emptied);
    }

    #[test]
    fn a_side_that_only_deleted_lines_is_not_required_to_survive() {
        let conflicted = "\
<<<<<<< HEAD
kept = 1
=======
>>>>>>> feature
";
        assert_eq!(
            judge(conflicted, "kept = 1\n"),
            ResolutionVerdict::Resolved,
            "a branch whose change was a deletion contributes no line to look for"
        );
    }

    #[test]
    fn a_line_both_sides_share_is_no_evidence_either_survived() {
        let conflicted = "\
<<<<<<< HEAD
shared
ours only
=======
shared
theirs only
>>>>>>> feature
";
        assert_eq!(
            judge(conflicted, "shared\n"),
            ResolutionVerdict::Discarded(ConflictSide::Ours),
            "keeping the line both branches already agreed on proves nothing"
        );
    }

    #[test]
    fn taking_one_side_of_one_hunk_while_merging_another_is_allowed() {
        let conflicted = "\
<<<<<<< HEAD
version = 1
=======
version = 2
>>>>>>> feature
body
<<<<<<< HEAD
ours feature
=======
theirs feature
>>>>>>> feature
";
        let resolved = "version = 2\nbody\nours feature\ntheirs feature\n";
        assert_eq!(
            judge(conflicted, resolved),
            ResolutionVerdict::Resolved,
            "a real merge picks a side per hunk; only losing a branch entirely is a discard"
        );
    }

    #[test]
    fn indentation_changes_do_not_read_as_a_discard() {
        let resolved = "\
fn greet() {
        println!(\"hello from ours\");
        let ours = 1;
        println!(\"hello from theirs\");
        let theirs = 2;
}
";
        assert_eq!(judge(CONFLICTED, resolved), ResolutionVerdict::Resolved);
    }

    #[test]
    fn an_unterminated_hunk_yields_nothing_rather_than_half_a_side() {
        let conflicted = "<<<<<<< HEAD\nours\n=======\ntheirs\n";
        assert!(
            hunks(conflicted).is_empty(),
            "a truncated conflict is not a conflict this can judge"
        );
    }

    #[test]
    fn verdicts_render_as_stable_identifiers() {
        assert_eq!(ResolutionVerdict::Resolved.as_str(), "resolved");
        assert_eq!(
            ResolutionVerdict::Discarded(ConflictSide::Ours).as_str(),
            "discarded_ours"
        );
        assert!(ResolutionVerdict::Resolved.accepted());
        assert!(!ResolutionVerdict::MarkersRemain.accepted());
    }

    /// A line that merely starts like a marker -- a longer underline, a
    /// banner -- is content, and belongs to whichever side carries it.
    #[test]
    fn a_line_longer_than_a_marker_is_content_of_its_side() {
        let conflicted = "\
<<<<<<< HEAD
Title
========
ours
=======
theirs
>>>>>>>>>> not a marker
more theirs
>>>>>>> feature
";
        let parsed = hunks(conflicted);

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].ours, vec!["Title", "========", "ours"]);
        assert_eq!(
            parsed[0].theirs,
            vec!["theirs", ">>>>>>>>>> not a marker", "more theirs"]
        );
        assert_eq!(
            judge(conflicted, "Title\n========\nours\ntheirs\nmore theirs\n"),
            ResolutionVerdict::Resolved,
            "an underline in the repaired file is not a leftover marker"
        );
    }
}
