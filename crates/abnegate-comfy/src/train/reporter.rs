use crate::train::Phase;
use crate::train::TrainProgress;
use std::sync::Arc;
use tokio::sync::watch;

/// Where a training run publishes its [`TrainProgress`], if anywhere, and the
/// step budget it will train on. Only an update that differs from the one
/// already published reaches the channel. A channel no one can read any more
/// never fails the run: a host that stops watching leaves the training to
/// finish.
#[derive(Debug, Clone, Default)]
pub(crate) struct Reporter {
    sender: Option<Arc<watch::Sender<Option<TrainProgress>>>>,
    total: u32,
    #[cfg(test)]
    published: Option<Arc<std::sync::Mutex<Vec<TrainProgress>>>>,
}

impl Reporter {
    pub(crate) fn new(sender: watch::Sender<Option<TrainProgress>>) -> Self {
        Self {
            sender: Some(Arc::new(sender)),
            ..Self::default()
        }
    }

    /// A reporter that keeps every update it publishes, in order, where a
    /// watch channel would keep only the last.
    #[cfg(test)]
    pub(crate) fn recording() -> (Self, Arc<std::sync::Mutex<Vec<TrainProgress>>>) {
        let published = Arc::new(std::sync::Mutex::new(Vec::new()));
        let reporter = Self {
            published: Some(Arc::clone(&published)),
            ..Self::new(watch::Sender::new(None))
        };
        (reporter, published)
    }

    /// The same reporter for a run that will train for `total` steps.
    pub(crate) fn with_total(mut self, total: u32) -> Self {
        self.total = total;
        self
    }

    pub(crate) fn total(&self) -> u32 {
        self.total
    }

    /// Whether anyone can still read an update: a receiver is open, or the
    /// host kept a sender of its own to subscribe one with later.
    pub(crate) fn listening(&self) -> bool {
        #[cfg(test)]
        if self.published.is_some() {
            return true;
        }
        self.sender
            .as_ref()
            .is_some_and(|sender| sender.receiver_count() > 0 || sender.sender_count() > 1)
    }

    /// Publishes `progress` unless it is the update already published.
    pub(crate) fn send(&self, progress: TrainProgress) {
        #[cfg(test)]
        let recorded = progress.clone();
        if let Some(sender) = &self.sender
            && publish(sender, progress)
        {
            #[cfg(test)]
            self.record(recorded);
        }
    }

    #[cfg(test)]
    fn record(&self, progress: TrainProgress) {
        if let Some(published) = &self.published {
            published
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(progress);
        }
    }

    /// Says the run is `done` of `count` items into `phase`, on the step
    /// budget it will train on and with nothing trained yet.
    pub(crate) fn phase(
        &self,
        phase: Phase,
        message: impl Into<String>,
        done: usize,
        count: usize,
    ) {
        if !self.listening() {
            return;
        }
        self.send(
            TrainProgress::new(0, self.total)
                .with_phase(phase)
                .with_message(message)
                .with_percent(percent(done, count)),
        );
    }

    /// Says the run is `done` of `count` steps into publishing what it
    /// trained, with every step of the budget the trainer last reported taken.
    pub(crate) fn publishing(&self, message: impl Into<String>, done: usize, count: usize) {
        if !self.listening() {
            return;
        }
        let total = self
            .sender
            .as_ref()
            .and_then(|sender| sender.borrow().as_ref().map(|progress| progress.total))
            .unwrap_or(self.total);
        self.send(
            TrainProgress::new(total, total)
                .with_phase(Phase::Publishing)
                .with_message(message)
                .with_percent(percent(done, count)),
        );
    }
}

fn publish(sender: &watch::Sender<Option<TrainProgress>>, progress: TrainProgress) -> bool {
    sender.send_if_modified(|current| {
        if current.as_ref() == Some(&progress) {
            return false;
        }
        *current = Some(progress);
        true
    })
}

fn percent(done: usize, count: usize) -> u8 {
    if count == 0 {
        return 0;
    }
    u8::try_from(done.min(count).saturating_mul(100) / count).unwrap_or(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phase_is_covered_from_empty_to_done() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(0, 8), 0);
        assert_eq!(percent(2, 8), 25);
        assert_eq!(percent(8, 8), 100);
        assert_eq!(percent(9, 8), 100);
    }

    #[test]
    fn a_phase_update_names_its_stage_and_the_training_budget() {
        let (sender, receiver) = watch::channel(None);
        Reporter::new(sender)
            .with_total(400)
            .phase(Phase::Screening, "Loading image 1 of 4", 1, 4);
        assert_eq!(
            *receiver.borrow(),
            Some(
                TrainProgress::new(0, 400)
                    .with_phase(Phase::Screening)
                    .with_message("Loading image 1 of 4")
                    .with_percent(25)
            )
        );
    }

    #[test]
    fn a_phase_before_the_budget_is_known_is_still_readable() {
        let (sender, receiver) = watch::channel(None);
        Reporter::new(sender).phase(Phase::Screening, "Screening the dataset", 0, 1);
        let update = receiver.borrow().clone().expect("the phase was published");
        assert_eq!(update.total, 1);
        assert_eq!(
            TrainProgress::parse(&serde_json::to_vec(&update).unwrap()),
            Some(update)
        );
    }

    #[test]
    fn an_update_identical_to_the_last_is_not_published_again() {
        let (sender, mut receiver) = watch::channel(None);
        let reporter = Reporter::new(sender);
        reporter.send(TrainProgress::new(1, 400));
        assert!(receiver.has_changed().unwrap());
        receiver.mark_unchanged();
        reporter.send(TrainProgress::new(1, 400));
        assert!(
            !receiver.has_changed().unwrap(),
            "a repeated update woke the watcher"
        );
        reporter.send(TrainProgress::new(2, 400));
        assert!(receiver.has_changed().unwrap());
    }

    #[test]
    fn publishing_counts_every_step_the_trainer_last_reported() {
        let (sender, receiver) = watch::channel(None);
        let reporter = Reporter::new(sender).with_total(400);
        reporter.send(TrainProgress::new(90, 120).with_phase(Phase::Training));
        reporter.publishing("Publishing the weight", 1, 2);
        assert_eq!(
            *receiver.borrow(),
            Some(
                TrainProgress::new(120, 120)
                    .with_phase(Phase::Publishing)
                    .with_message("Publishing the weight")
                    .with_percent(50)
            )
        );
    }

    #[test]
    fn a_receiver_that_is_gone_is_not_an_error() {
        let (sender, receiver) = watch::channel(None);
        let reporter = Reporter::new(sender);
        drop(receiver);
        assert!(!reporter.listening());
        reporter.phase(Phase::Training, "Starting training", 0, 1);
        reporter.send(TrainProgress::new(1, 400));
        assert!(!Reporter::default().listening());
    }

    #[test]
    fn a_host_that_kept_a_sender_can_still_subscribe() {
        let (sender, receiver) = watch::channel(None);
        let kept = sender.clone();
        let reporter = Reporter::new(sender);
        drop(receiver);
        assert!(reporter.listening());
        reporter.send(TrainProgress::new(3, 400));
        assert_eq!(*kept.subscribe().borrow(), Some(TrainProgress::new(3, 400)));
    }
}
