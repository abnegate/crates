use std::sync::Mutex;

use serde::de::DeserializeOwned;

use crate::modality::{AiError, ResponseFormat, TextProvider, TextRequest};

const STRUCTURED_TEMPERATURE: f64 = 0.0;
const DEFAULT_TEMPERATURE: f64 = 0.7;
const MAX_DETAIL_CHARACTERS: usize = 300;

/// One exchange with a model, as it happened.
#[derive(Debug, Clone)]
pub struct Exchange {
    pub provider: String,
    pub model: String,
    /// The schema asked for, when one was.
    pub schema: Option<String>,
    pub system_prompt: String,
    pub user_prompt: String,
    pub answer: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub attempts: u32,
}

impl Exchange {
    /// What this exchange cost, at the given rate per million tokens.
    ///
    /// A single rate cannot express the input/output split every model has, so
    /// this is indicative. The token counts beside it are exact when the
    /// provider reports them.
    pub fn cost_usd(&self, per_million_tokens: f64) -> f64 {
        f64::from(self.input_tokens + self.output_tokens) / 1_000_000.0 * per_million_tokens
    }
}

/// One way to ask a model something.
///
/// Owns provider choice, the retry policy, what gets recorded, and what
/// happens when there is no provider at all, which is a first-class answer
/// rather than a crash or a silent fabrication.
pub struct AiClient {
    provider: Option<Box<dyn TextProvider>>,
    /// How many times to ask again when the answer will not parse.
    retries: u32,
    exchanges: Mutex<Vec<Exchange>>,
}

impl AiClient {
    /// A client that will use `provider`.
    pub fn new(provider: Box<dyn TextProvider>) -> Self {
        Self {
            provider: Some(provider),
            retries: 1,
            exchanges: Mutex::new(Vec::new()),
        }
    }

    /// A client for a run that may not call a model.
    ///
    /// Every method answers [`AiError::NoProvider`], so a caller that wants to
    /// carry on without one can, and a caller that cannot gets a sentence
    /// explaining what is missing instead of a failure deep inside a parser.
    pub fn disabled() -> Self {
        Self {
            provider: None,
            retries: 0,
            exchanges: Mutex::new(Vec::new()),
        }
    }

    /// How many extra attempts a malformed answer gets. Default 1.
    pub fn with_retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }

    /// Whether this run can call a model at all.
    pub fn is_enabled(&self) -> bool {
        self.provider.is_some()
    }

    /// The provider's name, or `"none"`.
    pub fn provider_name(&self) -> &str {
        match &self.provider {
            Some(provider) => provider.name(),
            None => "none",
        }
    }

    /// The provider itself, for the callers that need the trait object.
    pub fn text_provider(&self) -> Option<&dyn TextProvider> {
        self.provider.as_deref()
    }

    /// How much context this provider can take, for chunking decisions.
    pub fn max_context_tokens(&self) -> u32 {
        self.provider
            .as_ref()
            .map_or(0, |provider| provider.max_context_tokens())
    }

    /// Every exchange so far.
    pub fn exchanges(&self) -> Vec<Exchange> {
        self.exchanges
            .lock()
            .map(|exchanges| exchanges.clone())
            .unwrap_or_default()
    }

    /// Total tokens in and out across the run.
    pub fn token_totals(&self) -> (u32, u32) {
        self.exchanges()
            .iter()
            .fold((0, 0), |(input, output), exchange| {
                (
                    input + exchange.input_tokens,
                    output + exchange.output_tokens,
                )
            })
    }

    /// Ask for prose.
    pub async fn complete(
        &self,
        wanted: &str,
        system_prompt: &str,
        user_prompt: &str,
        max_tokens: u32,
    ) -> Result<String, AiError> {
        let provider = self.provider(wanted)?;

        let request = TextRequest {
            system_prompt: system_prompt.to_string(),
            user_prompt: user_prompt.to_string(),
            temperature: DEFAULT_TEMPERATURE,
            max_tokens,
            response_format: None,
            context: None,
        };

        let response = provider.complete(&request).await?;
        self.record(Exchange {
            provider: provider.name().to_string(),
            model: response.model.clone(),
            schema: None,
            system_prompt: request.system_prompt,
            user_prompt: request.user_prompt,
            answer: response.content.clone(),
            input_tokens: response.input_tokens,
            output_tokens: response.output_tokens,
            attempts: 1,
        });

        Ok(response.content)
    }

    /// Ask for a value of a particular shape.
    ///
    /// `schema_name` is what the caller is asking for, in words, and appears in
    /// every error, so a failure says which step could not be completed rather
    /// than only that some JSON did not parse.
    ///
    /// An answer that does not deserialise is retried; one that still does not
    /// is an error. It is never half-read and never filled in with a default.
    pub async fn complete_structured<T: DeserializeOwned>(
        &self,
        schema_name: &str,
        schema: &serde_json::Value,
        system_prompt: &str,
        user_prompt: &str,
        max_tokens: u32,
    ) -> Result<T, AiError> {
        let provider = self.provider(schema_name)?;

        let request = TextRequest {
            system_prompt: system_prompt.to_string(),
            user_prompt: user_prompt.to_string(),
            temperature: STRUCTURED_TEMPERATURE,
            max_tokens,
            response_format: Some(ResponseFormat::Json {
                schema: Some(schema.clone()),
            }),
            context: None,
        };

        let mut last: Option<String> = None;
        for attempt in 1..=(self.retries + 1) {
            let value = match provider.complete_structured(&request).await {
                Ok(value) => value,
                Err(error) if attempt <= self.retries => {
                    last = Some(error.to_string());
                    continue;
                }
                Err(error) => return Err(AiError::Provider(error)),
            };

            let rendered = value.to_string();
            match serde_json::from_value(value) {
                Ok(parsed) => {
                    self.record(Exchange {
                        provider: provider.name().to_string(),
                        model: String::new(),
                        schema: Some(schema_name.to_string()),
                        system_prompt: request.system_prompt,
                        user_prompt: request.user_prompt,
                        answer: rendered,
                        input_tokens: 0,
                        output_tokens: 0,
                        attempts: attempt,
                    });
                    return Ok(parsed);
                }
                Err(error) => {
                    last = Some(format!(
                        "{error}; answer was {}",
                        rendered
                            .chars()
                            .take(MAX_DETAIL_CHARACTERS)
                            .collect::<String>()
                    ));
                }
            }
        }

        Err(AiError::Shape {
            schema: schema_name.to_string(),
            detail: last.unwrap_or_else(|| "no answer".into()),
        })
    }

    fn provider(&self, wanted: &str) -> Result<&dyn TextProvider, AiError> {
        self.provider.as_deref().ok_or_else(|| AiError::NoProvider {
            wanted: wanted.to_string(),
        })
    }

    fn record(&self, exchange: Exchange) {
        if let Ok(mut exchanges) = self.exchanges.lock() {
            exchanges.push(exchange);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::modality::vendor::MockProvider;

    const MISSING: &str = "/nonexistent";

    #[derive(Debug, Deserialize, PartialEq)]
    struct Facts {
        title: String,
        genre: String,
    }

    fn schema() -> serde_json::Value {
        serde_json::json!({ "title": "Facts" })
    }

    #[tokio::test]
    async fn a_run_with_no_provider_says_what_it_cannot_do() {
        let client = AiClient::disabled();

        let error = client
            .complete_structured::<Facts>("the compression plan", &schema(), "sys", "usr", 100)
            .await
            .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("the compression plan"), "{message}");
        assert!(message.contains("has none"), "{message}");
        assert!(message.len() < 120, "too wordy to repeat: {message}");
        assert!(!client.is_enabled());
        assert_eq!(client.provider_name(), "none");
        assert_eq!(client.max_context_tokens(), 0);
        assert!(client.text_provider().is_none());
    }

    #[tokio::test]
    async fn a_run_with_no_provider_declines_prose_too() {
        let client = AiClient::disabled();

        let error = client
            .complete("the summary", "sys", "usr", 100)
            .await
            .unwrap_err();

        assert!(matches!(error, AiError::NoProvider { .. }), "{error:?}");
    }

    #[tokio::test]
    async fn a_well_shaped_answer_is_returned_and_recorded() {
        let provider = MockProvider::new(MISSING)
            .with_answer("facts", r#"{"title": "Requiem", "genre": "RPG"}"#);
        let client = AiClient::new(Box::new(provider));

        let facts: Facts = client
            .complete_structured("the facts", &schema(), "sys", "usr", 100)
            .await
            .unwrap();

        assert_eq!(
            facts,
            Facts {
                title: "Requiem".into(),
                genre: "RPG".into(),
            }
        );
        let log = client.exchanges();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].schema.as_deref(), Some("the facts"));
        assert_eq!(log[0].provider, "mock");
        assert_eq!(log[0].attempts, 1);
    }

    #[tokio::test]
    async fn an_answer_of_the_wrong_shape_is_refused_not_patched() {
        let provider = MockProvider::new(MISSING).with_answer("facts", r#"{"title": "only"}"#);
        let client = AiClient::new(Box::new(provider));

        let error = client
            .complete_structured::<Facts>("the facts", &schema(), "sys", "usr", 100)
            .await
            .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("the facts"), "{message}");
        assert!(message.contains("discarded"), "{message}");
    }

    #[tokio::test]
    async fn a_malformed_answer_is_asked_for_again_before_giving_up() {
        let provider = MockProvider::new(MISSING).with_answer("facts", r#"{"title": "only"}"#);
        let client = AiClient::new(Box::new(provider)).with_retries(2);

        let error = client
            .complete_structured::<Facts>("the facts", &schema(), "sys", "usr", 100)
            .await
            .unwrap_err();

        assert!(matches!(error, AiError::Shape { .. }), "{error:?}");
        assert!(client.exchanges().is_empty());
    }

    #[tokio::test]
    async fn a_provider_failure_is_reported_as_one() {
        let client = AiClient::new(Box::new(MockProvider::new(MISSING)));

        let error = client
            .complete_structured::<Facts>("the facts", &schema(), "sys", "usr", 100)
            .await
            .unwrap_err();

        assert!(matches!(error, AiError::Provider(_)), "{error:?}");
    }

    #[tokio::test]
    async fn the_provider_name_reflects_what_actually_ran() {
        let client = AiClient::new(Box::new(MockProvider::new(MISSING)));

        assert_eq!(client.provider_name(), "mock");
        assert!(client.is_enabled());
        assert!(client.text_provider().is_some());
        assert_eq!(client.max_context_tokens(), 100_000);
    }

    #[tokio::test]
    async fn tokens_are_totalled_across_a_run() {
        let provider = MockProvider::new(MISSING)
            .with_answer("you-are-a-narrator", "Once upon a time there was a dragon.");
        let client = AiClient::new(Box::new(provider));

        let answer = client
            .complete("a bridge", "You are a narrator.", "Write a line.", 100)
            .await
            .unwrap();

        assert_eq!(answer, "Once upon a time there was a dragon.");
        let (input, output) = client.token_totals();
        assert!(output > 0, "the mock counts words, so output must be > 0");
        assert!(input > 0);
    }

    #[test]
    fn a_cost_is_derived_from_the_tokens_that_were_used() {
        let exchange = Exchange {
            provider: "anthropic".into(),
            model: "claude-opus-5".into(),
            schema: None,
            system_prompt: String::new(),
            user_prompt: String::new(),
            answer: String::new(),
            input_tokens: 500_000,
            output_tokens: 500_000,
            attempts: 1,
        };

        assert!((exchange.cost_usd(25.0) - 25.0).abs() < 1e-9);
    }
}
