use serde::Deserialize;
use serde::Serialize;

/// Exact capability advertised by a catalogue provider.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelCapability {
    Text,
    ImageInput,
    ImageGeneration,
    Audio,
    AudioInput,
    AudioGeneration,
    VideoInput,
    VideoGeneration,
    Tools,
    Embeddings,
    Reasoning,
}

pub(crate) fn push_capability(
    capabilities: &mut Vec<ModelCapability>,
    capability: ModelCapability,
) {
    if !capabilities.contains(&capability) {
        capabilities.push(capability);
    }
}

pub(crate) fn declared_capabilities<'a>(
    tags: impl Iterator<Item = &'a str>,
) -> Option<Vec<ModelCapability>> {
    use ModelCapability::*;

    let mut capabilities = Vec::new();
    for tag in tags {
        let declared: &[ModelCapability] = match tag {
            "text-generation"
            | "conversational"
            | "text2text-generation"
            | "summarization"
            | "translation"
            | "text-classification"
            | "token-classification"
            | "question-answering" => &[Text],
            "image-text-to-text" | "image-to-text" | "visual-question-answering" => {
                &[ImageInput, Text]
            }
            "text-to-image" => &[Text, ImageGeneration],
            "image-to-image" => &[ImageInput, ImageGeneration],
            "image-text-to-image" => &[Text, ImageInput, ImageGeneration],
            "unconditional-image-generation" => &[ImageGeneration],
            "image-classification"
            | "object-detection"
            | "image-segmentation"
            | "depth-estimation" => &[ImageInput],
            "text-to-audio" | "text-to-speech" => &[Text, AudioGeneration],
            "automatic-speech-recognition" => &[AudioInput, Text],
            "audio-classification" => &[AudioInput],
            "audio-to-audio" => &[AudioInput, AudioGeneration],
            "text-to-video" => &[Text, VideoGeneration],
            "image-to-video" => &[ImageInput, VideoGeneration],
            "video-text-to-text" => &[VideoInput, Text],
            "video-classification" => &[VideoInput],
            "feature-extraction" | "sentence-similarity" | "embedding" | "embeddings" => {
                &[Embeddings]
            }
            "tools" => &[Tools],
            "thinking" => &[Reasoning],
            "vision" => &[ImageInput],
            "audio" => &[Audio],
            _ => &[],
        };
        for &capability in declared {
            push_capability(&mut capabilities, capability);
        }
    }
    (!capabilities.is_empty()).then_some(capabilities)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_capabilities_map_known_tasks() {
        assert_eq!(
            declared_capabilities(["text-to-image", "feature-extraction"].into_iter()),
            Some(vec![
                ModelCapability::Text,
                ModelCapability::ImageGeneration,
                ModelCapability::Embeddings
            ])
        );
        assert_eq!(declared_capabilities(["unknown-task"].into_iter()), None);
        assert_eq!(
            declared_capabilities(["image-to-image"].into_iter()),
            Some(vec![
                ModelCapability::ImageInput,
                ModelCapability::ImageGeneration
            ])
        );
    }

    #[test]
    fn push_capability_deduplicates() {
        let mut capabilities = vec![ModelCapability::Text];
        push_capability(&mut capabilities, ModelCapability::Text);
        push_capability(&mut capabilities, ModelCapability::Tools);
        assert_eq!(
            capabilities,
            vec![ModelCapability::Text, ModelCapability::Tools]
        );
    }
}
