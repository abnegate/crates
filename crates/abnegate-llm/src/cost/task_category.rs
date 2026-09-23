use serde::{Deserialize, Serialize};

/// The kind of output a task asks for, which decides the models that can do it.
///
/// Every [`ModelPricing`](crate::cost::ModelPricing) entry is filed under one
/// of these, so a model is only ever offered for the work it actually does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TaskCategory {
    Text,
    Image,
    Voice,
    Music,
    Model3D,
    Video,
    Embedding,
    Transcription,
}

#[cfg(test)]
mod tests {
    use super::TaskCategory;

    #[test]
    fn every_category_round_trips_through_json() {
        for category in [
            TaskCategory::Text,
            TaskCategory::Image,
            TaskCategory::Voice,
            TaskCategory::Music,
            TaskCategory::Model3D,
            TaskCategory::Video,
            TaskCategory::Embedding,
            TaskCategory::Transcription,
        ] {
            let json = serde_json::to_string(&category).unwrap();
            assert_eq!(
                serde_json::from_str::<TaskCategory>(&json).unwrap(),
                category
            );
        }
    }
}
