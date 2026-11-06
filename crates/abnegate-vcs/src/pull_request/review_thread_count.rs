/// How many of a pull request's review threads are still open, and how many
/// someone resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct ReviewThreadCount {
    /// Threads nobody has resolved yet.
    pub open: u32,
    /// Threads someone marked resolved.
    pub resolved: u32,
}

impl ReviewThreadCount {
    /// `open` unresolved threads and `resolved` resolved ones.
    pub fn new(open: u32, resolved: u32) -> Self {
        Self { open, resolved }
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
            .fold(ReviewThreadCount::default(), ReviewThreadCount::with);

        assert_eq!(count, ReviewThreadCount::new(2, 3));
    }

    #[test]
    fn a_count_never_overflows() {
        let full = ReviewThreadCount::new(u32::MAX, u32::MAX);

        assert_eq!(full.with(false).with(true), full);
    }
}
