use crate::config::ConfigError;
use std::time::Duration;

const PICKUP: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const STEPS_PER_IMAGE: u32 = 20;
const MINIMUM_STEPS: u32 = 500;
const MAXIMUM_STEPS: u32 = 8000;

/// A trainer running beside ComfyUI on the host, which picks up the jobs
/// [`lora::train`](crate::lora::train) queues under
/// [`Contract::host_directory`](crate::train::Contract::host_directory) and
/// writes their progress and weights back there.
///
/// Set on [`Config::host_trainer`](crate::Config::host_trainer), it trains
/// what the packaged graph cannot, when no
/// [`Config::train_command`](crate::Config::train_command) is set: every method
/// other than a LoRA, and every SDXL base.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Trainer {
    /// How long a queued job may wait for the trainer to start it before the
    /// run fails, two minutes by default, counted from the last moment
    /// another job was running.
    pub pickup: Duration,
    /// Wait between two reads of a job's files, two seconds by default.
    pub poll_interval: Duration,
    /// Training steps the trainer takes per image, 20 by default.
    pub steps_per_image: u32,
    /// Fewest steps the trainer takes on any set, 500 by default.
    pub minimum_steps: u32,
    /// Most steps the trainer takes on any set, 8000 by default.
    pub maximum_steps: u32,
}

impl Default for Trainer {
    fn default() -> Self {
        Self {
            pickup: PICKUP,
            poll_interval: POLL_INTERVAL,
            steps_per_image: STEPS_PER_IMAGE,
            minimum_steps: MINIMUM_STEPS,
            maximum_steps: MAXIMUM_STEPS,
        }
    }
}

impl Trainer {
    /// A trainer with the default pickup, poll interval and step budget.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets how long a queued job may wait for the trainer to start it.
    pub fn with_pickup(mut self, pickup: Duration) -> Self {
        self.pickup = pickup;
        self
    }

    /// Sets the wait between two reads of a job's files.
    pub fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }

    /// Sets the step budget the trainer is configured with: `per_image`
    /// steps for every image, held between `minimum` and `maximum`.
    pub fn with_steps(mut self, per_image: u32, minimum: u32, maximum: u32) -> Self {
        self.steps_per_image = per_image;
        self.minimum_steps = minimum;
        self.maximum_steps = maximum;
        self
    }

    /// The steps the trainer takes on `image_count` images, which a queued
    /// job reports as its total before the trainer has said otherwise.
    pub fn steps_for(&self, image_count: usize) -> u32 {
        u32::try_from(image_count)
            .unwrap_or(u32::MAX)
            .saturating_mul(self.steps_per_image)
            .max(self.minimum_steps)
            .min(self.maximum_steps)
    }

    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.pickup.is_zero() || self.poll_interval.is_zero() {
            return Err(ConfigError::new(
                "the host trainer's pickup and poll interval must be longer than zero",
            ));
        }
        if self.maximum_steps == 0 || self.minimum_steps > self.maximum_steps {
            return Err(ConfigError::new(
                "the host trainer's step budget must allow at least one step",
            ));
        }
        Ok(())
    }
}
