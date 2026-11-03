use serde::Deserialize;
use serde::Serialize;

/// The stage a training run is in, as a [`TrainProgress`](crate::train::TrainProgress)
/// reports it.
///
/// A trainer names its stage on the wire as a lowercase string, read without
/// regard to surrounding whitespace or ASCII case. A name this crate does not
/// know is kept, trimmed, in [`Phase::Other`] rather than refused, so a newer
/// trainer's stage still reaches the host.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
#[non_exhaustive]
pub enum Phase {
    /// Waiting for a trainer to pick the job up.
    Queued,
    /// Decoding, screening and cropping the uploaded images.
    Screening,
    /// Asking the vision model to caption the images left blank.
    Captioning,
    /// Turning a dataset into the format the trainer reads.
    Converting,
    /// Loading the base model.
    Loading,
    /// Generating the class images a prior-preservation run trains beside.
    ClassImages,
    /// Encoding the dataset into latents.
    Encoding,
    /// Running the training loop.
    Training,
    /// Writing the trained weight where it is served from.
    Publishing,
    /// A stage this crate has no name for, under the name the trainer gave it.
    Other(String),
}

impl Phase {
    /// The phase named `name` on the wire, trimmed and in any ASCII case.
    pub fn parse(name: &str) -> Self {
        let name = name.trim();
        match name.to_ascii_lowercase().as_str() {
            "queued" => Self::Queued,
            "screening" => Self::Screening,
            "captioning" => Self::Captioning,
            "converting" => Self::Converting,
            "loading" => Self::Loading,
            "class_images" => Self::ClassImages,
            "encoding" => Self::Encoding,
            "training" => Self::Training,
            "publishing" => Self::Publishing,
            _ => Self::Other(name.to_string()),
        }
    }

    /// The name the phase travels under on the wire.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Queued => "queued",
            Self::Screening => "screening",
            Self::Captioning => "captioning",
            Self::Converting => "converting",
            Self::Loading => "loading",
            Self::ClassImages => "class_images",
            Self::Encoding => "encoding",
            Self::Training => "training",
            Self::Publishing => "publishing",
            Self::Other(name) => name,
        }
    }
}

impl From<String> for Phase {
    fn from(name: String) -> Self {
        Self::parse(&name)
    }
}

impl From<Phase> for String {
    fn from(phase: Phase) -> Self {
        match phase {
            Phase::Other(name) => name,
            known => known.as_str().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIRE: &[(Phase, &str)] = &[
        (Phase::Queued, "queued"),
        (Phase::Screening, "screening"),
        (Phase::Captioning, "captioning"),
        (Phase::Converting, "converting"),
        (Phase::Loading, "loading"),
        (Phase::ClassImages, "class_images"),
        (Phase::Encoding, "encoding"),
        (Phase::Training, "training"),
        (Phase::Publishing, "publishing"),
    ];

    #[test]
    fn every_phase_keeps_its_wire_name() {
        for (phase, name) in WIRE {
            assert_eq!(phase.as_str(), *name);
            assert_eq!(Phase::parse(name), *phase);
            assert_eq!(serde_json::to_value(phase).unwrap(), *name);
            assert_eq!(
                serde_json::from_value::<Phase>(serde_json::json!(name)).unwrap(),
                *phase
            );
        }
    }

    #[test]
    fn a_known_phase_is_read_in_any_case_and_padding() {
        for (phase, name) in WIRE {
            for written in [
                name.to_ascii_uppercase(),
                format!("  {name}\n"),
                format!(" {}", name.to_ascii_uppercase()),
            ] {
                assert_eq!(Phase::parse(&written), *phase, "{written:?}");
                assert_eq!(Phase::from(written.clone()), *phase, "{written:?}");
            }
        }
        assert_eq!(
            Phase::from(" Distilling ".to_string()),
            Phase::Other("Distilling".into())
        );
    }

    #[test]
    fn a_phase_this_crate_does_not_know_keeps_the_trainers_name() {
        let phase: Phase = serde_json::from_value(serde_json::json!("distilling")).unwrap();
        assert_eq!(phase, Phase::Other("distilling".into()));
        assert_eq!(phase.as_str(), "distilling");
        assert_eq!(serde_json::to_value(&phase).unwrap(), "distilling");
    }
}
