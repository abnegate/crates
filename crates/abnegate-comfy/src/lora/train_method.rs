use serde::Deserialize;
use serde::Serialize;

/// How a training set becomes a weight, which decides where a
/// [`TrainPolicy`](crate::lora::TrainPolicy) publishes it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TrainMethod {
    /// A low-rank adapter loaded on top of its base.
    #[default]
    Lora,
    /// A full checkpoint fine-tuned from its base.
    Finetune,
    /// A low-rank adapter trained together with a new token embedding for its
    /// trigger, published and loaded like a [`Lora`](Self::Lora). Only a
    /// training command or the host trainer trains one.
    Pivotal,
    /// A low-rank adapter for a video recipe, trained on windows cut from the
    /// request's clips rather than on its photos. Only the host trainer
    /// trains one, from the job's
    /// [`CLIPS_DIRECTORY`](crate::host::CLIPS_DIRECTORY).
    Video,
}

impl TrainMethod {
    /// The name the method travels under on the wire and in a trainer's
    /// environment.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lora => "lora",
            Self::Finetune => "finetune",
            Self::Pivotal => "pivotal",
            Self::Video => "video",
        }
    }
}
