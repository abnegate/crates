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

use crate::conflict::{BASE_MARKER, OURS_MARKER, SPLIT_MARKER, THEIRS_MARKER};

/// Which branch's work a repair dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictSide {
    Ours,
    Theirs,
}

impl ConflictSide {
    pub fn as_str(self) -> &'static str {
        match self {
            ConflictSide::Ours => "ours",
            ConflictSide::Theirs => "theirs",
        }
    }
}

impl std::fmt::Display for ConflictSide {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One `<<<<<<< / ======= / >>>>>>>` block, split into the two sides it offers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConflictHunk {
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
}

/// What a repaired file is, judged against the conflicted file it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionVerdict {
    Resolved,
    NoConflict,
    MarkersRemain,
    Emptied,
    Discarded(ConflictSide),
}

impl ResolutionVerdict {
    pub fn accepted(self) -> bool {
        self == ResolutionVerdict::Resolved
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ResolutionVerdict::Resolved => "resolved",
            ResolutionVerdict::NoConflict => "no_conflict",
            ResolutionVerdict::MarkersRemain => "markers_remain",
            ResolutionVerdict::Emptied => "emptied",
            ResolutionVerdict::Discarded(ConflictSide::Ours) => "discarded_ours",
            ResolutionVerdict::Discarded(ConflictSide::Theirs) => "discarded_theirs",
        }
    }
}

impl std::fmt::Display for ResolutionVerdict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Region {
    Outside,
    Ours,
    Base,
    Theirs,
}

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
        if line.starts_with(OURS_MARKER) {
            current = ConflictHunk::default();
            region = Region::Ours;
        } else if line.starts_with(BASE_MARKER) && region == Region::Ours {
            region = Region::Base;
        } else if line.starts_with(SPLIT_MARKER) && matches!(region, Region::Ours | Region::Base) {
            region = Region::Theirs;
        } else if line.starts_with(THEIRS_MARKER) && region == Region::Theirs {
            hunks.push(std::mem::take(&mut current));
            region = Region::Outside;
        } else {
            match region {
                Region::Ours => current.ours.push(line.to_string()),
                Region::Theirs => current.theirs.push(line.to_string()),
                Region::Base | Region::Outside => {}
            }
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
fn distinctive<'a>(side: &'a [String], other: &[String]) -> Vec<&'a str> {
    let shared = significant(other);
    let mut unique: Vec<&str> = significant(side)
        .into_iter()
        .filter(|line| !shared.contains(line))
        .collect();
    unique.sort_unstable();
    unique.dedup();
    unique
}

fn survives(resolved: &str, lines: &[&str]) -> bool {
    lines
        .iter()
        .any(|line| resolved.lines().any(|candidate| candidate.trim() == *line))
}

/// Judge a repaired file against the conflicted file it was produced from.
pub fn judge(conflicted: &str, resolved: &str) -> ResolutionVerdict {
    let hunks = hunks(conflicted);
    if hunks.is_empty() {
        return ResolutionVerdict::NoConflict;
    }

    if resolved
        .lines()
        .any(|line| line.starts_with(OURS_MARKER) || line.starts_with(THEIRS_MARKER))
    {
        return ResolutionVerdict::MarkersRemain;
    }

    if resolved.trim().is_empty() && !conflicted.trim().is_empty() {
        return ResolutionVerdict::Emptied;
    }

    let mut ours: Vec<&str> = Vec::new();
    let mut theirs: Vec<&str> = Vec::new();
    for hunk in &hunks {
        ours.extend(distinctive(&hunk.ours, &hunk.theirs));
        theirs.extend(distinctive(&hunk.theirs, &hunk.ours));
    }

    if !ours.is_empty() && !survives(resolved, &ours) {
        return ResolutionVerdict::Discarded(ConflictSide::Ours);
    }
    if !theirs.is_empty() && !survives(resolved, &theirs) {
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
}
