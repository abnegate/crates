use crate::host;
use crate::host::Trainer;

/// What a host job trains on, staged inside its attempt before the job
/// directory takes it over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Handoff {
    /// The image set and its captions and companions, staged as `targets`.
    Images,
}

impl Handoff {
    /// The directory inside the attempt the set is staged in.
    pub(super) fn staged(self) -> &'static str {
        match self {
            Self::Images => "targets",
        }
    }

    /// The directory inside the job the trainer reads the set from.
    pub(super) fn directory(self) -> &'static str {
        match self {
            Self::Images => host::DATASET_DIRECTORY,
        }
    }

    /// The steps `trainer` takes on a set of `count` images.
    pub(super) fn steps(self, trainer: &Trainer, count: usize) -> u32 {
        match self {
            Self::Images => trainer.steps_for(count),
        }
    }
}
