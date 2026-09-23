use chrono::DateTime;
use chrono::Utc;

/// Whole minutes between two RFC 3339 instants, or `None` if either is unreadable.
pub fn minutes_between(opened: &str, merged: &str) -> Option<i64> {
    let opened = DateTime::parse_from_rfc3339(opened).ok()?;
    let merged = DateTime::parse_from_rfc3339(merged).ok()?;
    Some((merged.with_timezone(&Utc) - opened.with_timezone(&Utc)).num_minutes())
}

/// How a change was received once the agent handed it over.
///
/// Timestamps stay RFC 3339 and tallies stay plain counts, because this is read
/// back out of a run's artifacts by the learning loop rather than by a person.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PullRequestReception {
    pub opened_at: Option<String>,
    pub merged_at: Option<String>,
    pub minutes_to_merge: Option<i64>,
    pub review_cycles: u32,
    pub approvals: u32,
    pub state: Option<String>,
    pub comments: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_minutes_are_whole_and_reject_unreadable_timestamps() {
        assert_eq!(
            minutes_between("2026-09-04T09:00:00Z", "2026-09-04T11:30:00Z"),
            Some(150)
        );
        assert_eq!(
            minutes_between("2026-09-04T09:00:00+02:00", "2026-09-04T09:00:00Z"),
            Some(120),
            "offsets must be normalised before subtracting"
        );
        assert_eq!(minutes_between("soon", "2026-09-04T11:30:00Z"), None);
        assert_eq!(minutes_between("2026-09-04T09:00:00Z", ""), None);
    }
}
