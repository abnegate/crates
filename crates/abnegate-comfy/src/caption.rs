//! Vision captioning for LoRA training sets.
//!
//! An identity LoRA learns its subject from the trigger word, so a caption must
//! describe only what changes between shots. A caption that restates the subject
//! teaches the model to rebuild it from the words instead, and prompting the
//! trigger alone then renders something unrelated. Captioning therefore runs in
//! two passes: name the subject shared by every image, then describe each image
//! while excluding that subject.

mod draft;
mod image;
mod request;

pub use draft::Draft;
pub use image::CaptionImage;
pub use request::CaptionRequest;

use abnegate_llm::{LlmClient, LlmConfig, Message};
use abnegate_secret::SecretValue;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use crate::config::Config;

const SUBJECT_TOKENS: u32 = 40;
const DESCRIPTION_TOKENS: u32 = 80;
const MAXIMUM_CAPTION_WORDS: usize = 18;
/// A word in at least this share of the descriptions is invariant, so it is identity.
/// Set low on purpose: leaking identity costs more than dropping a little context.
const INVARIANT_SHARE: f32 = 0.34;
/// Shown to anchor the answer format. Small models copy it verbatim, so it is
/// also the one answer that is never accepted.
const EXAMPLE_CAPTION: &str =
    "close-up from the side, sitting on a wooden stool, warm indoor light";

const PREAMBLES: &[&str] = &[
    "in this image,",
    "in the image,",
    "in this photo,",
    "in the photo,",
    "this image shows",
    "this image depicts",
    "this photo shows",
    "the image shows",
    "the image depicts",
    "the photo shows",
    "here is",
    "this is",
    "a photo of",
    "a picture of",
    "an image of",
    "image of",
    "picture of",
    "photo of",
];

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "the", "with", "of", "in", "on", "at", "for", "to", "its", "it", "that",
    "this", "has", "have", "is", "are", "was", "were", "one", "two", "some", "very", "small",
    "large", "big", "tiny",
];

/// Inline data URL, the only image shape an OpenAI-compatible vision model takes.
pub fn data_url(filename: &str, base64: &str) -> String {
    let extension = Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    };
    format!("data:{mime};base64,{}", base64.trim())
}

pub struct Captioner {
    model: String,
    timeout: Duration,
    host: String,
    key: SecretValue,
}

impl Captioner {
    pub fn new(config: &Config, litellm_host: String, litellm_key: SecretValue) -> Self {
        Self {
            model: config.caption_model.clone(),
            timeout: Duration::from_secs(config.caption_timeout_seconds),
            host: litellm_host,
            key: litellm_key,
        }
    }

    pub fn available(&self) -> bool {
        !self.host.trim().is_empty() && !self.model.trim().is_empty()
    }

    /// Caption every image whose caption is blank. User-written captions are
    /// kept, and a group is looked at once however many images it holds.
    ///
    /// Returns the descriptions the vision model produced, before identity
    /// words are stripped from them, so a caller can measure how much of that
    /// vocabulary the set has in common.
    pub async fn fill(&self, drafts: &mut [Draft], trigger: &str) -> Vec<String> {
        if !self.available() || drafts.iter().all(|draft| !draft.caption.trim().is_empty()) {
            return Vec::new();
        }
        let subject = self.subject(&drafts[0].image).await;
        let mut shots: Vec<usize> = Vec::new();
        let mut described: Vec<Option<String>> = Vec::new();
        for draft in drafts.iter() {
            if !draft.caption.trim().is_empty() || shots.contains(&draft.group) {
                continue;
            }
            shots.push(draft.group);
            described.push(self.describe(&draft.image, subject.as_deref()).await);
        }
        let banned = identity_words(&described, subject.as_deref(), trigger);
        let vocabulary: Vec<String> = described.iter().flatten().cloned().collect();
        let mut seen: HashSet<String> = HashSet::new();
        let captions: Vec<String> = described
            .into_iter()
            .map(|draft| {
                let kept = draft
                    .map(|value| strip_words(&value, &banned))
                    .filter(|value| !value.is_empty())
                    .filter(|value| seen.insert(value.to_ascii_lowercase()));
                match kept {
                    Some(value) if trigger.is_empty() => value,
                    Some(value) => format!("{trigger}, {value}"),
                    None => trigger.to_string(),
                }
            })
            .collect();
        for draft in drafts.iter_mut() {
            if !draft.caption.trim().is_empty() {
                continue;
            }
            if let Some(shot) = shots.iter().position(|&group| group == draft.group) {
                draft.caption = captions[shot].clone();
            }
        }
        vocabulary
    }

    /// One noun phrase naming the subject, used to seed the exclusion list.
    async fn subject(&self, image: &str) -> Option<String> {
        let prompt = "What is the main object in this photo? Answer with a short noun phrase, \
             at most 12 words. Do not write a sentence.";
        let mut message = Message::user(prompt);
        message.images = vec![image.to_string()];
        let answer = self.ask(message, SUBJECT_TOKENS).await?;
        let cleaned = tidy(&answer);
        (!cleaned.is_empty()).then_some(cleaned)
    }

    /// What varies in one image: pose, framing, setting, lighting, props.
    async fn describe(&self, image: &str, subject: Option<&str>) -> Option<String> {
        let exclusion = match subject {
            Some(subject) => format!(" Do not name or describe the {subject} itself."),
            None => String::new(),
        };
        let prompt = format!(
            "Write a short caption for this photo. Say where it was taken, how it is framed, \
             and what the lighting is like.{exclusion} Answer with lowercase phrases separated \
             by commas.\nExample answer: {EXAMPLE_CAPTION}"
        );
        let mut message = Message::user(prompt);
        message.images = vec![image.to_string()];
        let answer = self.ask(message, DESCRIPTION_TOKENS).await?;
        let cleaned = tidy(&answer);
        if cleaned.is_empty() || cleaned.eq_ignore_ascii_case(EXAMPLE_CAPTION) {
            return None;
        }
        Some(cleaned)
    }

    async fn ask(&self, message: Message, max_tokens: u32) -> Option<String> {
        let client = LlmClient::new(
            LlmConfig::new(self.host.clone(), self.model.clone(), self.key.clone())
                .with_temperature(0.0)
                .with_max_tokens(max_tokens),
        );
        let result = tokio::time::timeout(
            self.timeout,
            client.chat_with_model(&self.model, &[message], None),
        )
        .await;
        let Ok(Ok(response)) = result else {
            return None;
        };
        response
            .choices
            .first()
            .and_then(|choice| choice.message.content.clone())
    }
}

/// Words that identify the subject rather than the shot.
///
/// Small vision models ignore "do not describe the subject", so the subject
/// phrase alone is not enough. A word repeated across most descriptions cannot
/// be describing what varies between them, so it is identity too.
fn identity_words(
    drafts: &[Option<String>],
    subject: Option<&str>,
    trigger: &str,
) -> HashSet<String> {
    let mut banned: HashSet<String> = HashSet::new();
    for source in subject.into_iter().chain(std::iter::once(trigger)) {
        banned.extend(content_words(source));
    }
    let described: Vec<HashSet<String>> = drafts
        .iter()
        .flatten()
        .map(|draft| content_words(draft).collect())
        .collect();
    if described.len() < 3 {
        return banned;
    }
    let threshold = ((described.len() as f32 * INVARIANT_SHARE).ceil() as usize).max(3);
    let mut counts: HashMap<&String, usize> = HashMap::new();
    for words in &described {
        for word in words {
            *counts.entry(word).or_default() += 1;
        }
    }
    banned.extend(
        counts
            .into_iter()
            .filter(|(_, count)| *count >= threshold)
            .map(|(word, _)| word.clone()),
    );
    banned
}

pub(crate) fn content_words(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split(|character: char| !character.is_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|word| word.len() > 2 && !STOPWORDS.contains(&word.as_str()))
}

/// Drop the comma clauses that carry identity, keeping the ones about the shot.
fn strip_words(description: &str, banned: &HashSet<String>) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut budget = MAXIMUM_CAPTION_WORDS;
    for clause in description.split(',').map(str::trim) {
        if clause.is_empty() || content_words(clause).any(|word| banned.contains(&word)) {
            continue;
        }
        let length = clause.split_whitespace().count();
        if length > budget {
            break;
        }
        budget -= length;
        kept.push(clause);
    }
    kept.join(", ")
}

/// Strip model chatter, markdown, and preambles from a raw answer.
fn tidy(answer: &str) -> String {
    let mut value = answer.trim().replace(['\n', '\r'], " ");
    value = value.replace(['*', '`', '"'], "");
    value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut lowered = value.to_ascii_lowercase();
    loop {
        let trimmed = PREAMBLES
            .iter()
            .find(|preamble| lowered.starts_with(**preamble))
            .map(|preamble| preamble.len());
        match trimmed {
            Some(length) => {
                value = value[length..].trim_start().to_string();
                lowered = value.to_ascii_lowercase();
            }
            None => break,
        }
    }
    value
        .trim_end_matches(['.', ',', ';'])
        .trim()
        .trim_start_matches(['-', ':'])
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn config(model: &str) -> Config {
        Config {
            caption_model: model.to_string(),
            caption_timeout_seconds: 5,
            ..Default::default()
        }
    }

    fn answer(content: &str) -> serde_json::Value {
        serde_json::json!({
            "id": "caption",
            "object": "chat.completion",
            "created": 0,
            "model": "vision",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": content},
                "finish_reason": "stop"
            }]
        })
    }

    #[test]
    fn images_print_their_size_rather_than_their_bytes() {
        let upload = "A".repeat(1 << 20);
        let image = CaptionImage {
            filename: "a.png".into(),
            bytes_base64: upload.clone(),
            caption: String::new(),
            group: None,
        };
        let draft = Draft::new("a.png", &upload, "", 0);
        for rendered in [format!("{image:?}"), format!("{draft:?}")] {
            assert!(rendered.len() < 200, "{} characters", rendered.len());
        }
    }

    #[test]
    fn tidy_strips_preamble_markdown_and_trailing_punctuation() {
        assert_eq!(
            tidy("The image shows **a robot on concrete**."),
            "a robot on concrete"
        );
        assert_eq!(
            tidy("In this image, standing outdoors;"),
            "standing outdoors"
        );
        assert_eq!(
            tidy("\"a photo of side profile, walking\""),
            "side profile, walking"
        );
    }

    #[test]
    fn subject_words_are_removed_from_captions() {
        let banned = identity_words(&[], Some("a glossy red teapot robot"), "zrkxyz");
        assert_eq!(
            strip_words(
                "teapot robot on a plinth, three-quarter view, standing on concrete",
                &banned
            ),
            "three-quarter view, standing on concrete"
        );
    }

    /// A small vision model ignores "do not describe the subject" and names it in
    /// every answer, so words repeated across the set are identity as well.
    #[test]
    fn words_repeated_across_the_set_are_treated_as_identity() {
        let drafts: Vec<Option<String>> = [
            "toy robot with a camera head, with a steam train in the background",
            "whimsical robot on a city street, under a gray sky",
            "3d model of a whimsical robot, the background is neutral",
            "toy robot with a camera head, posed on a white surface",
        ]
        .iter()
        .map(|value| Some(value.to_string()))
        .collect();
        let banned = identity_words(&drafts, Some("robot tea kettle"), "zrkxyz");
        assert!(banned.contains("robot"), "robot is in every description");
        assert!(!banned.contains("street"), "street varies between shots");
        assert_eq!(
            strip_words(drafts[0].as_deref().unwrap(), &banned),
            "with a steam train in the background"
        );
    }

    #[test]
    fn captions_are_capped_without_cutting_a_clause_in_half() {
        let clause = "standing on a wooden stool in a warehouse under warm light";
        let description = format!("{clause}, {clause}");
        let capped = strip_words(&description, &HashSet::new());
        assert_eq!(
            capped, clause,
            "a clause that does not fit is dropped whole"
        );
        assert!(capped.split_whitespace().count() <= MAXIMUM_CAPTION_WORDS);
    }

    #[tokio::test]
    async fn fill_captions_blank_entries_and_keeps_written_ones() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_string_contains("main object in this photo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer(
                "a lime-green cube-headed robot with a glossy red teapot body",
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_string_contains("Write a short caption"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer(
                "The image shows a lime-green teapot robot, three-quarter view, standing on concrete.",
            )))
            .mount(&server)
            .await;
        let captioner = Captioner::new(&config("vision"), server.uri(), "key".into());
        let mut images = vec![
            Draft::new("a.png", "aaa", "", 0),
            Draft::new("b.png", "bbb", "hand written", 1),
        ];
        captioner.fill(&mut images, "zrkxyz").await;

        assert_eq!(
            images[1].caption, "hand written",
            "written captions must survive"
        );
        let generated = &images[0].caption;
        assert!(generated.starts_with("zrkxyz, "), "got {generated}");
        assert!(
            !generated.contains("teapot") && !generated.contains("robot"),
            "subject leaked into {generated}"
        );
        assert!(
            generated.contains("concrete"),
            "context dropped from {generated}"
        );
    }

    #[tokio::test]
    async fn a_model_that_repeats_one_answer_falls_back_to_the_trigger() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_string_contains("main object in this photo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer("a teapot robot")))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_string_contains("Write a short caption"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer(
                "camera angle, framing, pose or action, background or setting, lighting",
            )))
            .mount(&server)
            .await;
        let captioner = Captioner::new(&config("vision"), server.uri(), "key".into());
        let mut images = vec![
            Draft::new("a.png", "aaa", "", 0),
            Draft::new("b.png", "bbb", "", 1),
            Draft::new("c.png", "ccc", "", 2),
        ];
        captioner.fill(&mut images, "zrkxyz").await;
        for (index, draft) in images.iter().enumerate() {
            assert_eq!(
                draft.caption, "zrkxyz",
                "image {index}: one answer repeated for every image describes nothing that varies"
            );
        }
    }

    /// Stripping identity words is what destroys the overlap between captions,
    /// so the set can only be judged on the answers as the model gave them.
    #[tokio::test]
    async fn fill_returns_descriptions_with_the_shared_words_intact() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_string_contains("main object in this photo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer("a teapot robot")))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_string_contains("Write a short caption"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer(
                "a teapot robot on a plinth, three-quarter view, standing on concrete",
            )))
            .mount(&server)
            .await;
        let captioner = Captioner::new(&config("vision"), server.uri(), "key".into());
        let mut images = vec![
            Draft::new("a.png", "aaa", "", 0),
            Draft::new("b.png", "bbb", "", 1),
            Draft::new("c.png", "ccc", "hand written", 2),
        ];
        let described = captioner.fill(&mut images, "zrkxyz").await;

        assert_eq!(
            described.len(),
            2,
            "one description per image the model saw"
        );
        assert!(
            described.iter().all(|value| value.contains("robot")),
            "the shared word must survive for the set to be measurable: {described:?}"
        );
        assert!(
            !images[0].caption.contains("robot"),
            "the caption itself still drops identity: {}",
            images[0].caption
        );
    }

    #[tokio::test]
    async fn the_litellm_key_reaches_the_authorization_header_intact() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer sk-litellm-0123456789"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer("a teapot robot")))
            .expect(2)
            .mount(&server)
            .await;
        let captioner = Captioner::new(
            &config("vision"),
            server.uri(),
            SecretValue::new("sk-litellm-0123456789"),
        );
        let mut images = vec![Draft::new("a.png", "aaa", "", 0)];
        captioner.fill(&mut images, "zrkxyz").await;
    }

    #[tokio::test]
    async fn fill_is_a_no_op_without_a_caption_model() {
        let captioner = Captioner::new(&config(""), "http://unused".into(), "key".into());
        let mut images = vec![Draft::new("a.png", "aaa", "", 0)];
        captioner.fill(&mut images, "zrkxyz").await;
        assert_eq!(images[0].caption, "");
    }

    #[tokio::test]
    async fn unreachable_vision_model_leaves_the_trigger_only_caption() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let captioner = Captioner::new(&config("vision"), server.uri(), "key".into());
        let mut images = vec![Draft::new("a.png", "aaa", "", 0)];
        captioner.fill(&mut images, "zrkxyz").await;
        assert_eq!(images[0].caption, "zrkxyz");
    }

    #[tokio::test]
    async fn one_shot_is_described_once_however_many_frames_it_holds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_string_contains("main object in this photo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer("a teapot robot")))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_string_contains("Write a short caption"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(answer("three-quarter view, standing on concrete")),
            )
            .mount(&server)
            .await;
        let captioner = Captioner::new(&config("vision"), server.uri(), "key".into());
        let mut frames: Vec<Draft> = (0..6)
            .map(|index| Draft::new(&format!("{index}.png"), "aaa", "", index / 3))
            .collect();
        captioner.fill(&mut frames, "zrkxyz").await;

        let described = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| {
                String::from_utf8_lossy(&request.body).contains("Write a short caption")
            })
            .count();
        assert_eq!(described, 2, "six frames of two shots cost two round trips");
        assert_eq!(frames[0].caption, frames[2].caption, "a shot reads alike");
        assert!(
            frames
                .iter()
                .all(|frame| frame.caption.starts_with("zrkxyz")),
            "every frame still carries the trigger"
        );
    }
}
