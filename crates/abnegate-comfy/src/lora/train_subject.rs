use serde::Deserialize;
use serde::Serialize;

/// What a training set shows, which decides the bases a
/// [`TrainPolicy`](crate::lora::TrainPolicy) lets it train on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TrainSubject {
    /// Anything that is not a person: an object, a character, a style.
    #[default]
    Other,
    /// One real person, whose identity the weight has to keep.
    Person,
}

impl TrainSubject {
    /// The name the subject travels under on the wire and in a trainer's
    /// environment.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Other => "other",
            Self::Person => "person",
        }
    }
}
