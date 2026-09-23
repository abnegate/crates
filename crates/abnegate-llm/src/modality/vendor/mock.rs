//! A text provider that answers from files on disk.
//!
//! It never invents an answer: asked something it has no file for, it says so
//! and names the file it looked for, so a missing fixture fails here rather
//! than producing a plausible answer further downstream.

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use futures::Stream;

use crate::modality::{
    ResponseFormat, StructuredResponse, TextProvider, TextRequest, TextResponse,
};
use crate::provider::ProviderError;

const MAX_CONTEXT_TOKENS: u32 = 100_000;
const MAX_KEY_CHARACTERS: usize = 60;

/// Answers keyed by the schema or prompt they belong to.
pub struct MockProvider {
    root: PathBuf,
    inline: HashMap<String, String>,
}

impl MockProvider {
    /// Read answers from `root`, one `.json` or `.txt` file per key.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            inline: HashMap::new(),
        }
    }

    /// Add an answer without a file, for a test that wants one inline.
    pub fn with_answer(mut self, key: &str, answer: impl Into<String>) -> Self {
        self.inline.insert(key.to_string(), answer.into());
        self
    }

    /// The key a request is answered under.
    ///
    /// A schema names what is being asked for better than a prompt does:
    /// prompts change constantly, while the shape of the answer is stable.
    /// Falls back to the first line of the system prompt.
    pub fn key_for(request: &TextRequest) -> String {
        if let Some(ResponseFormat::Json {
            schema: Some(schema),
        }) = &request.response_format
        {
            if let Some(title) = schema.get("title").and_then(|title| title.as_str()) {
                return slug(title);
            }
            if let Some(properties) = schema.get("properties").and_then(|value| value.as_object()) {
                let mut names: Vec<&str> = properties.keys().map(String::as_str).collect();
                names.sort_unstable();
                if !names.is_empty() {
                    return slug(&names.join("-"));
                }
            }
        }

        let first_line = request
            .system_prompt
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default();
        if !first_line.is_empty() {
            return slug(first_line);
        }
        slug(request.user_prompt.lines().next().unwrap_or("prompt"))
    }

    fn lookup(&self, key: &str) -> Result<String, ProviderError> {
        if let Some(answer) = self.inline.get(key) {
            return Ok(answer.clone());
        }

        for extension in ["json", "txt"] {
            let path = self.root.join(format!("{key}.{extension}"));
            if path.exists() {
                return std::fs::read_to_string(&path).map_err(|error| {
                    ProviderError::config(format!("could not read {}: {error}", path.display()))
                });
            }
        }

        Err(ProviderError::config(format!(
            "the mock provider has no answer for '{key}'. Add {} with the reply this request \
             should get. Mock answers are never invented, so a missing one fails here rather \
             than further downstream.",
            self.root.join(format!("{key}.json")).display()
        )))
    }
}

fn words(text: &str) -> u32 {
    u32::try_from(text.split_whitespace().count()).unwrap_or(u32::MAX)
}

/// A file name that is recognisable from the thing it answers.
///
/// `NarrativeAnalysis` becomes `narrative-analysis`: word boundaries in
/// CamelCase count as separators, not just punctuation.
fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    let mut previous_was_lower = false;

    for character in text.chars().take(MAX_KEY_CHARACTERS) {
        if character.is_ascii_alphanumeric() {
            if character.is_ascii_uppercase() && previous_was_lower && !last_dash {
                out.push('-');
            }
            out.extend(character.to_lowercase());
            last_dash = false;
            previous_was_lower = character.is_ascii_lowercase() || character.is_ascii_digit();
        } else {
            if !last_dash {
                out.push('-');
                last_dash = true;
            }
            previous_was_lower = false;
        }
    }

    out.trim_matches('-').to_string()
}

#[async_trait]
impl TextProvider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn supports_structured_output(&self) -> bool {
        true
    }

    fn max_context_tokens(&self) -> u32 {
        MAX_CONTEXT_TOKENS
    }

    async fn complete(&self, request: &TextRequest) -> Result<TextResponse, ProviderError> {
        let key = Self::key_for(request);
        let content = self.lookup(&key)?;
        Ok(TextResponse {
            output_tokens: words(&content),
            input_tokens: words(&request.system_prompt).saturating_add(words(&request.user_prompt)),
            content,
            model: format!("mock:{key}"),
            finish_reason: "end_turn".into(),
        })
    }

    async fn complete_structured(
        &self,
        request: &TextRequest,
    ) -> Result<StructuredResponse, ProviderError> {
        let key = Self::key_for(request);
        let raw = self.lookup(&key)?;
        let value = serde_json::from_str(&raw).map_err(|error| {
            ProviderError::parse(format!(
                "the mock answer for '{key}' is not valid JSON: {error}. A mock answer for a \
                 structured request must be the value itself."
            ))
        })?;
        Ok(StructuredResponse {
            value,
            model: format!("mock:{key}"),
            input_tokens: words(&request.system_prompt).saturating_add(words(&request.user_prompt)),
            output_tokens: words(&raw),
        })
    }

    async fn stream_complete(
        &self,
        request: &TextRequest,
    ) -> Result<Box<dyn Stream<Item = Result<String, ProviderError>> + Send + Unpin>, ProviderError>
    {
        let response = self.complete(request).await?;
        Ok(Box::new(futures::stream::iter(vec![Ok(response.content)])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MISSING: &str = "/nonexistent";

    fn request_with_schema(title: &str) -> TextRequest {
        TextRequest {
            system_prompt: "You are the StoryAnalyst.".into(),
            user_prompt: "Analyse this.".into(),
            temperature: 0.0,
            max_tokens: 1024,
            response_format: Some(ResponseFormat::Json {
                schema: Some(serde_json::json!({ "title": title })),
            }),
            context: None,
        }
    }

    #[tokio::test]
    async fn an_answer_is_keyed_by_the_shape_it_must_take() {
        let provider = MockProvider::new(MISSING)
            .with_answer("narrative-analysis", r#"{"mandatory_beats": []}"#);

        let structured = provider
            .complete_structured(&request_with_schema("NarrativeAnalysis"))
            .await
            .unwrap();

        assert!(structured.value.get("mandatory_beats").is_some());
        assert_eq!(structured.model, "mock:narrative-analysis");
    }

    #[tokio::test]
    async fn a_missing_answer_names_the_file_it_wanted() {
        let provider = MockProvider::new("/fixtures");

        let error = provider
            .complete_structured(&request_with_schema("SceneAnalysis"))
            .await
            .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("scene-analysis"), "{message}");
        assert!(message.contains("/fixtures"), "{message}");
    }

    #[tokio::test]
    async fn it_never_invents_an_answer() {
        let provider = MockProvider::new(MISSING);
        assert!(
            provider
                .complete(&request_with_schema("Anything"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_structured_answer_that_is_not_json_is_an_error() {
        let provider =
            MockProvider::new(MISSING).with_answer("narrative-analysis", "not json at all");

        let error = provider
            .complete_structured(&request_with_schema("NarrativeAnalysis"))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("not valid JSON"), "{error}");
    }

    #[tokio::test]
    async fn answers_come_from_files_on_disk() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("beat-plan.json"), r#"{"beats": 3}"#).unwrap();

        let provider = MockProvider::new(directory.path());
        let structured = provider
            .complete_structured(&request_with_schema("BeatPlan"))
            .await
            .unwrap();

        assert_eq!(structured.value["beats"], 3);
    }

    #[tokio::test]
    async fn a_streamed_answer_is_the_whole_answer_in_one_chunk() {
        use futures::StreamExt as _;

        let provider = MockProvider::new(MISSING).with_answer("you-are-a-narrator", "Once.");
        let mut request = TextRequest::new("You are a narrator.", "Write a line.");
        request.response_format = None;

        let chunks: Vec<String> = provider
            .stream_complete(&request)
            .await
            .unwrap()
            .map(|chunk| chunk.unwrap())
            .collect()
            .await;

        assert_eq!(chunks, vec!["Once.".to_string()]);
    }

    #[test]
    fn a_camel_case_name_reads_as_words() {
        assert_eq!(slug("NarrativeAnalysis"), "narrative-analysis");
        assert_eq!(slug("BeatPlan"), "beat-plan");
        assert_eq!(slug("Phase1Result"), "phase1-result");
        assert_eq!(slug("already-dashed"), "already-dashed");
        assert_eq!(
            slug("You are the SystemsAnalyst."),
            "you-are-the-systems-analyst"
        );
    }

    #[test]
    fn a_schema_without_a_title_is_keyed_by_its_fields() {
        let mut request = TextRequest::new("", "x");
        request.response_format = Some(ResponseFormat::Json {
            schema: Some(serde_json::json!({ "properties": { "beats": {}, "arcs": {} } })),
        });

        assert_eq!(MockProvider::key_for(&request), "arcs-beats");
    }

    #[test]
    fn a_plain_request_is_keyed_by_the_role_it_asks_for() {
        let request = TextRequest::new("You are the SystemsAnalyst.\nMore detail here.", "go");

        assert_eq!(
            MockProvider::key_for(&request),
            "you-are-the-systems-analyst"
        );
    }

    #[test]
    fn the_same_request_always_gets_the_same_key() {
        let first = MockProvider::key_for(&request_with_schema("NarrativeAnalysis"));
        let second = MockProvider::key_for(&request_with_schema("NarrativeAnalysis"));
        assert_eq!(first, second);
    }

    #[test]
    fn the_provider_reports_what_it_can_do() {
        let provider = MockProvider::new(MISSING);
        assert_eq!(provider.name(), "mock");
        assert!(provider.supports_structured_output());
        assert_eq!(provider.max_context_tokens(), MAX_CONTEXT_TOKENS);
    }
}
