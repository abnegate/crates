/// How many of a pull request's review threads are still open, how many
/// someone resolved, and whether that is every thread on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct ReviewThreadCount {
    /// Threads nobody has resolved yet.
    pub open: u32,
    /// Threads someone marked resolved.
    pub resolved: u32,
    /// Whether every thread on the pull request was counted. When it is
    /// `false` the read stopped short, and a thread it never reached may be
    /// one still open.
    pub complete: bool,
}

impl ReviewThreadCount {
    /// `open` unresolved threads and `resolved` resolved ones, every thread
    /// on the pull request when `complete`.
    pub fn new(open: u32, resolved: u32, complete: bool) -> Self {
        Self {
            open,
            resolved,
            complete,
        }
    }

    /// This count with one more thread, resolved or not.
    #[must_use]
    pub(crate) fn with(self, resolved: bool) -> Self {
        if resolved {
            Self {
                resolved: self.resolved.saturating_add(1),
                ..self
            }
        } else {
            Self {
                open: self.open.saturating_add(1),
                ..self
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_thread_counts_as_open_or_resolved() {
        let count = [false, true, false, true, true]
            .into_iter()
            .fold(ReviewThreadCount::new(0, 0, true), ReviewThreadCount::with);

        assert_eq!(count, ReviewThreadCount::new(2, 3, true));
    }

    #[test]
    fn a_count_never_overflows() {
        let full = ReviewThreadCount::new(u32::MAX, u32::MAX, true);

        assert_eq!(full.with(false).with(true), full);
    }

    #[test]
    fn a_count_nothing_was_read_for_does_not_claim_to_be_complete() {
        assert!(!ReviewThreadCount::default().complete);
    }
}
