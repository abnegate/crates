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
}

impl TrainMethod {
    /// The name the method travels under on the wire and in a trainer's
    /// environment.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lora => "lora",
            Self::Finetune => "finetune",
        }
    }
}
