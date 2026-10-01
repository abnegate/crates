//! Host-owned persistence for trials.

use crate::error::Result;
use crate::trial::Trial;

/// Host-owned persistence for trials.
///
/// This crate does not open a database. A host implements [`Archive`] against
/// its own store and hands it to [`Memory::restore`](crate::Memory::restore)
/// and [`Memory::record_into`](crate::Memory::record_into).
pub trait Archive: Send + Sync {
    /// Persist `trial` and return the id the store assigned.
    fn save(&mut self, trial: &Trial) -> Result<i64>;

    /// Every previously recorded trial, oldest first.
    fn load(&self) -> Result<Vec<Trial>>;

    /// Persist `lesson` on the trial `id`.
    fn save_lesson(&mut self, id: i64, lesson: &str) -> Result<()>;

    /// Persist `embedding` on the trial `id`.
    fn save_embedding(&mut self, id: i64, embedding: &[f32]) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::memory::Memory;
    use crate::trial_input::TrialInput;
    use crate::verdict::Verdict;
    use std::sync::Mutex;

    struct VecArchive {
        trials: Mutex<Vec<Trial>>,
    }

    impl VecArchive {
        fn new() -> Self {
            Self {
                trials: Mutex::new(Vec::new()),
            }
        }
    }

    impl Archive for VecArchive {
        fn save(&mut self, trial: &Trial) -> Result<i64> {
            let mut trials = self
                .trials
                .lock()
                .map_err(|error| Error::archive(error.to_string()))?;
            if let Some(existing) = trials.iter_mut().find(|item| item.id == trial.id) {
                *existing = trial.clone();
            } else {
                trials.push(trial.clone());
            }
            Ok(trial.id)
        }

        fn load(&self) -> Result<Vec<Trial>> {
            self.trials
                .lock()
                .map(|trials| trials.clone())
                .map_err(|error| Error::archive(error.to_string()))
        }

        fn save_lesson(&mut self, id: i64, lesson: &str) -> Result<()> {
            let mut trials = self
                .trials
                .lock()
                .map_err(|error| Error::archive(error.to_string()))?;
            if let Some(trial) = trials.iter_mut().find(|trial| trial.id == id) {
                trial.lesson = Some(lesson.to_string());
            }
            Ok(())
        }

        fn save_embedding(&mut self, id: i64, embedding: &[f32]) -> Result<()> {
            let mut trials = self
                .trials
                .lock()
                .map_err(|error| Error::archive(error.to_string()))?;
            if let Some(trial) = trials.iter_mut().find(|trial| trial.id == id) {
                trial.embedding = Some(embedding.to_vec());
            }
            Ok(())
        }
    }

    #[test]
    fn restore_replays_saved_trials_with_their_ids() {
        let mut archive = VecArchive::new();
        let mut first = Memory::new();
        first.record(
            TrialInput::new("lab", "fuzz")
                .with_id(7)
                .with_verdict(Verdict::Skip)
                .with_error("reprl-unavailable"),
        );
        first.persist(&mut archive).unwrap();

        let mut second = Memory::new();
        second.restore(&archive).unwrap();
        assert_eq!(second.get(7).unwrap().strategy, "fuzz");
        let id = second.record(TrialInput::new("lab", "jit").with_verdict(Verdict::Empty));
        assert_eq!(id, 8);
    }

    #[test]
    fn record_into_and_attach_persist_through_the_archive() {
        let mut archive = VecArchive::new();
        let mut memory = Memory::new();
        let id = memory
            .record_into(
                TrialInput::new("lab", "fuzz").with_verdict(Verdict::Skip),
                &mut archive,
            )
            .unwrap();
        memory
            .attach_lesson_into(id, "fuzz cannot run without reprl", &mut archive)
            .unwrap();
        memory
            .attach_embedding_into(id, vec![1.0, 0.0], &mut archive)
            .unwrap();

        let mut restored = Memory::new();
        restored.restore(&archive).unwrap();
        let trial = restored.get(id).unwrap();
        assert_eq!(
            trial.lesson.as_deref(),
            Some("fuzz cannot run without reprl")
        );
        assert_eq!(trial.embedding.as_deref(), Some(&[1.0, 0.0][..]));
    }
}
