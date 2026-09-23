//! What a change is called, in the one format this history already uses.

use std::fmt;

mod kind;

pub use crate::subject::kind::Kind;

/// Longest a subject line may run, prefix included.
///
/// A subject is read in a list of subjects, so what does not fit on one line
/// of that list is not read at all.
const MAXIMUM_LINE_LENGTH: usize = 72;

/// What a cut summary may not end on.
const TRAILING: [char; 2] = [' ', '.'];

/// What a change with no summary is called. A task title is only `NOT NULL`,
/// so an empty one reaches here.
const UNTITLED: &str = "apply the work of a background task";

/// A commit subject: the kind of change, then what it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
    kind: Kind,
    summary: String,
}

impl Subject {
    /// A subject of `kind` saying `summary`, made one line that fits the
    /// subject's budget with its prefix, or saying where the change came from
    /// when `summary` says nothing.
    pub fn new(kind: Kind, summary: &str) -> Self {
        let summary = normalize(summary, kind);
        Self {
            kind,
            summary: match summary.is_empty() {
                true => normalize(UNTITLED, kind),
                false => summary,
            },
        }
    }

    /// The subject a change falls back to when nothing classified it.
    ///
    /// A task's title is what the user asked for rather than what the run
    /// turned out to do, so it stands in for a summary without claiming to be
    /// one. A title with nothing in it leaves the subject saying only where the
    /// change came from, which still beats a kind with nothing after it.
    pub fn unclassified(title: &str) -> Self {
        Self::new(Kind::UNCLASSIFIED, title)
    }

    /// Read `(kind): summary` back out of a line.
    ///
    /// A classifier answers in the format it was asked for or it does not
    /// answer: a line this cannot read is one the caller falls back from,
    /// rather than one it prefixes a guessed kind onto. A reply that runs on
    /// past one line is a reply that ignored the format, and the explanation
    /// trailing it would otherwise be folded into the subject.
    pub fn parse(line: &str) -> Option<Self> {
        let mut lines = line.trim().lines();
        let line = lines.next()?;
        lines.next().is_none().then_some(())?;
        let (kind, summary) = line.trim().trim_start_matches('(').split_once("):")?;
        let kind = Kind::parse(kind)?;
        let summary = normalize(summary, kind);
        (!summary.is_empty()).then_some(Self { kind, summary })
    }

    /// The kind of change announced.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// What the change did.
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "({}): {}", self.kind, self.summary)
    }
}

/// The characters a summary of `kind` has left once `(kind): ` is written.
fn budget(kind: Kind) -> usize {
    MAXIMUM_LINE_LENGTH.saturating_sub(format!("({kind}): ").chars().count())
}

/// One line within the budget for `kind`, no trailing stop or space, and
/// starting lowercase where that does not change a word that was already
/// capitalised for its own sake.
fn normalize(summary: &str, kind: Kind) -> String {
    let collapsed = summary.split_whitespace().collect::<Vec<&str>>().join(" ");
    let trimmed = collapsed.trim_end_matches(TRAILING);
    let opening = trimmed.split_whitespace().next().unwrap_or_default();
    let lowered = match opening.chars().skip(1).any(char::is_uppercase) {
        true => trimmed.to_string(),
        false => {
            let mut characters = trimmed.chars();
            match characters.next() {
                Some(first) => first.to_lowercase().chain(characters).collect(),
                None => String::new(),
            }
        }
    };
    lowered
        .chars()
        .take(budget(kind))
        .collect::<String>()
        .trim_end_matches(TRAILING)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A classifier that explains itself did not answer in the format it was
    /// asked for. Folding the explanation into the summary named the change
    /// after the model's aside; falling back names it after the task.
    #[test]
    fn a_reply_that_runs_past_one_line_is_not_a_subject() {
        assert_eq!(
            Subject::parse("(fix): validate input\nExplanation: ..."),
            None
        );
        assert_eq!(
            Subject::parse("Here is the subject:\n(fix): validate input"),
            None
        );
        assert_eq!(
            Subject::parse("  (fix): validate input  \n\n"),
            Some(Subject::new(Kind::Fix, "validate input")),
            "trailing blank lines are not a second line"
        );
    }

    #[test]
    fn a_subject_renders_in_the_format_this_history_uses() {
        let subject = Subject::new(Kind::Fix, "stop the worker retrying a terminal failure");

        assert_eq!(
            subject.to_string(),
            "(fix): stop the worker retrying a terminal failure"
        );
    }

    /// A subject is read next to other subjects, so it is one line, starts the
    /// way the rest of them start, and does not end mid-sentence.
    #[test]
    fn a_summary_is_collapsed_lowercased_and_stripped_of_its_full_stop() {
        assert_eq!(
            Subject::new(Kind::Feat, "  Add\n a  rate   limit. ").summary(),
            "add a rate limit"
        );
        assert_eq!(
            Subject::new(Kind::Feat, "Add a limit...").summary(),
            "add a limit"
        );
    }

    /// Lowercasing the first letter is a house convention, not a rename: a word
    /// that carries its own capitals keeps them.
    #[test]
    fn a_word_capitalised_for_its_own_sake_is_left_alone() {
        assert_eq!(
            Subject::new(Kind::Fix, "PostgreSQL connections leak").summary(),
            "PostgreSQL connections leak"
        );
        assert_eq!(
            Subject::new(Kind::Fix, "GitHub rejects the push").summary(),
            "GitHub rejects the push"
        );
    }

    /// The budget is the line's, not the summary's: a summary cut to the
    /// whole budget ran the line past it by the length of its prefix.
    #[test]
    fn a_subject_past_the_line_it_is_read_on_is_cut_to_it() {
        for kind in Kind::ALL {
            let long = Subject::new(kind, &"a".repeat(200));

            assert_eq!(
                long.to_string().chars().count(),
                MAXIMUM_LINE_LENGTH,
                "{long}"
            );
        }
    }

    #[test]
    fn a_cut_never_ends_on_a_space_or_a_stop() {
        for filler in ["a b. c", "word. ", "ends here . "] {
            let long = Subject::new(Kind::Refactor, &filler.repeat(40));

            assert!(!long.summary().ends_with(TRAILING), "{long}");
            assert!(long.to_string().chars().count() <= MAXIMUM_LINE_LENGTH);
        }
    }

    #[test]
    fn a_subject_with_nothing_to_say_says_where_it_came_from() {
        for summary in ["", "   ", "..."] {
            assert_eq!(Subject::new(Kind::Fix, summary).summary(), UNTITLED);
        }
    }

    #[test]
    fn every_kind_round_trips_through_its_label() {
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.label()), Some(kind), "{kind}");
            assert_eq!(
                Kind::parse(&kind.label().to_uppercase()),
                Some(kind),
                "{kind}"
            );
        }
        assert_eq!(Kind::parse("improvement"), None);
    }

    #[test]
    fn a_classified_line_is_read_back_into_its_parts() {
        let subject = Subject::parse("(feat): give every tool a declared action tier").unwrap();

        assert_eq!(subject.kind(), Kind::Feat);
        assert_eq!(subject.summary(), "give every tool a declared action tier");
        assert_eq!(
            Subject::parse("feat): add a limit").unwrap().kind(),
            Kind::Feat,
            "a classifier that drops the opening bracket still answered the question"
        );
    }

    /// A line this cannot read is one the caller falls back from. Guessing a
    /// kind and keeping the rest would announce a change nobody classified.
    #[test]
    fn a_line_that_is_not_a_subject_is_refused_rather_than_guessed_at() {
        assert_eq!(Subject::parse("add a rate limit"), None);
        assert_eq!(Subject::parse("(improvement): add a rate limit"), None);
        assert_eq!(Subject::parse("(feat):"), None);
        assert_eq!(Subject::parse("(feat):   "), None);
        assert_eq!(Subject::parse(""), None);
    }

    /// Nothing classified this, so the kind claims the least it can while the
    /// title still says which task it came from.
    #[test]
    fn an_unclassified_change_falls_back_to_the_kind_that_claims_least() {
        let subject = Subject::unclassified("Add rate limiting to the public API");

        assert_eq!(subject.kind(), Kind::Chore);
        assert_eq!(
            subject.to_string(),
            "(chore): add rate limiting to the public API"
        );
    }

    /// A task's title is only `NOT NULL`, so an empty one reaches the fallback.
    /// `(chore): ` with nothing after it is not a subject.
    #[test]
    fn an_untitled_change_is_still_named_something_a_reader_can_read() {
        for title in ["", "   ", "\n\t "] {
            let subject = Subject::unclassified(title);

            assert_eq!(subject.summary(), UNTITLED, "{title:?}");
            assert_eq!(
                subject.to_string(),
                format!("(chore): {UNTITLED}"),
                "{title:?}"
            );
        }
    }
}
