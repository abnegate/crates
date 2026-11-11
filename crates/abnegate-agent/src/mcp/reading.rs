use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;

use tokio::task;
use tokio::task::Id;

/// Which recorded request a [`Remote`](super::remote::Remote) sent last, and
/// the task that reads the event stream it was answered with, once one has.
///
/// The transport of `rmcp` 3.9.0 reads each request's event stream in a
/// task of its own, apart from the task that holds its background stream
/// open, and resumes a request's stream from the task that read it, after
/// the last event it read. A stream that task opens is therefore that
/// request's, while one opened from any other task is not, whatever event
/// it resumes after.
///
/// That layout is `rmcp`'s own, which it neither documents nor promises.
/// The tests of a resumption challenged in a listing's place, and of a
/// background stream challenged while a listing's answer is read, guard
/// it, so re-check it and re-run them on every `rmcp` bump.
#[derive(Debug, Default)]
pub(super) struct Reading {
    request: u64,
    task: Option<Id>,
}

impl Reading {
    /// `reading`, locked, though a panic poisoned it.
    pub(super) fn locked(reading: &Mutex<Self>) -> MutexGuard<'_, Self> {
        reading.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The request sent now, whose answer no task has read yet.
    pub(super) fn sent(&mut self) -> u64 {
        self.request += 1;
        self.task = None;
        self.request
    }

    /// Note that the current task read the answer to `request`, if that is
    /// still the request sent last.
    pub(super) fn read(&mut self, request: u64) {
        if self.request == request {
            self.task = task::try_id();
        }
    }

    /// The request sent last, when the current task is the one that read
    /// its answer.
    pub(super) fn resumed(&self) -> Option<u64> {
        let current = task::try_id()?;
        (self.task == Some(current)).then_some(self.request)
    }

    /// Whether `request` is still the one sent last.
    pub(super) fn current(&self, request: u64) -> bool {
        self.request == request
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn nothing_read_outside_a_task_is_resumed() {
        let mut reading = Reading::default();
        let request = reading.sent();

        reading.read(request);

        assert_eq!(reading.resumed(), None);
    }

    #[tokio::test]
    async fn only_the_task_that_read_the_last_request_resumes_it() {
        let reading = Arc::new(Mutex::new(Reading::default()));
        let request = Reading::locked(&reading).sent();
        let reader = reading.clone();
        let resumed = tokio::spawn(async move {
            Reading::locked(&reader).read(request);
            Reading::locked(&reader).resumed()
        })
        .await
        .expect("the reading task");
        let elsewhere = {
            let reader = reading.clone();
            tokio::spawn(async move { Reading::locked(&reader).resumed() })
                .await
                .expect("another task")
        };

        assert_eq!(resumed, Some(request));
        assert_eq!(elsewhere, None);
    }

    #[tokio::test]
    async fn a_request_sent_after_forgets_who_read_the_one_before() {
        let reading = Arc::new(Mutex::new(Reading::default()));
        let earlier = Reading::locked(&reading).sent();
        let reader = reading.clone();
        let (resumed, current) = tokio::spawn(async move {
            Reading::locked(&reader).read(earlier);
            Reading::locked(&reader).sent();
            Reading::locked(&reader).read(earlier);
            let reading = Reading::locked(&reader);
            (reading.resumed(), reading.current(earlier))
        })
        .await
        .expect("the reading task");

        assert_eq!(resumed, None);
        assert!(!current);
    }
}
