use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use abnegate_http::Backoff;
use serde::de::DeserializeOwned;

use crate::modality::{AiError, Exchange, ResponseFormat, TextProvider, TextRequest};
#[cfg(doc)]
use crate::provider::ProviderError;

const STRUCTURED_TEMPERATURE: f64 = 0.0;
const DEFAULT_TEMPERATURE: f64 = 0.7;
const MAX_DETAIL_CHARACTERS: usize = 300;
const DEFAULT_RETRIES: u32 = 1;
const BACKOFF_BASE: Duration = Duration::from_secs(1);
const BACKOFF_MAXIMUM: Duration = Duration::from_secs(30);

/// One way to ask a model something.
///
/// Owns provider choice, the retry policy, what gets recorded, and what
/// happens when there is no provider at all, which is a first-class answer
/// rather than a crash or a silent fabrication.
///
/// A structured request is asked again when its answer does not fit the
/// schema, and when the provider fails in a way [`ProviderError::transient`]
/// says another attempt could survive, after an exponential backoff with
/// jitter. A refusal such as a rejected key is returned at once: asking again
/// would only be refused again.
pub struct AiClient {
    provider: Option<Box<dyn TextProvider>>,
    retries: u32,
    backoff: Backoff,
    exchanges: Mutex<Vec<Exchange>>,
}

impl fmt::Debug for AiClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AiClient")
            .field("provider", &self.provider_name())
            .field("retries", &self.retries)
            .field("backoff", &self.backoff)
            .finish_non_exhaustive()
    }
}

impl AiClient {
    /// A client that will use `provider`.
    pub fn new(provider: Box<dyn TextProvider>) -> Self {
        Self {
            provider: Some(provider),
            retries: DEFAULT_RETRIES,
            backoff: Backoff {
                base: BACKOFF_BASE,
                maximum: BACKOFF_MAXIMUM,
                ..Backoff::default()
            },
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
            backoff: Backoff::default(),
            exchanges: Mutex::new(Vec::new()),
        }
    }

    /// How many extra attempts a structured request gets. Default 1.
    pub fn with_retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }

    /// How long to wait before each retry of a recoverable provider failure.
    pub fn with_backoff(mut self, backoff: Backoff) -> Self {
        self.backoff = backoff;
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
    pub fn token_totals(&self) -> (u64, u64) {
        self.exchanges()
            .iter()
            .fold((0_u64, 0_u64), |(input, output), exchange| {
                (
                    input.saturating_add(u64::from(exchange.input_tokens)),
                    output.saturating_add(u64::from(exchange.output_tokens)),
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
        let mut input_tokens = 0_u32;
        let mut output_tokens = 0_u32;
        for attempt in 1..=self.retries.saturating_add(1) {
            let response = match provider.complete_structured(&request).await {
                Ok(response) => response,
                Err(error) if attempt <= self.retries && error.transient() => {
                    last = Some(error.to_string());
                    tokio::time::sleep(self.backoff.delay(attempt - 1)).await;
                    continue;
                }
                Err(error) => return Err(AiError::Provider(error)),
            };
            input_tokens = input_tokens.saturating_add(response.input_tokens);
            output_tokens = output_tokens.saturating_add(response.output_tokens);

            let rendered = response.value.to_string();
            match serde_json::from_value(response.value) {
                Ok(parsed) => {
                    self.record(Exchange {
                        provider: provider.name().to_string(),
                        model: response.model,
                        schema: Some(schema_name.to_string()),
                        system_prompt: request.system_prompt,
                        user_prompt: request.user_prompt,
                        answer: rendered,
                        input_tokens,
                        output_tokens,
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
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU32, Ordering};

    use async_trait::async_trait;
    use futures::Stream;
    use serde::Deserialize;

    use super::*;
    use crate::modality::vendor::MockProvider;
    use crate::modality::{StructuredResponse, TextResponse};
    use crate::provider::ProviderError;

    /// Answers each structured request with the next scripted result and
    /// counts the calls it received.
    struct Scripted {
        answers: Mutex<VecDeque<Result<StructuredResponse, ProviderError>>>,
        calls: std::sync::Arc<AtomicU32>,
    }

    impl Scripted {
        fn new(
            answers: impl IntoIterator<Item = Result<StructuredResponse, ProviderError>>,
        ) -> (Self, std::sync::Arc<AtomicU32>) {
            let calls = std::sync::Arc::new(AtomicU32::new(0));
            (
                Self {
                    answers: Mutex::new(answers.into_iter().collect()),
                    calls: calls.clone(),
                },
                calls,
            )
        }
    }

    #[async_trait]
    impl TextProvider for Scripted {
        fn name(&self) -> &str {
            "scripted"
        }

        fn supports_structured_output(&self) -> bool {
            true
        }

        fn max_context_tokens(&self) -> u32 {
            0
        }

        async fn complete(&self, _request: &TextRequest) -> Result<TextResponse, ProviderError> {
            Err(ProviderError::unsupported("complete"))
        }

        async fn complete_structured(
            &self,
            _request: &TextRequest,
        ) -> Result<StructuredResponse, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(ProviderError::unsupported("no answer scripted")))
        }

        async fn stream_complete(
            &self,
            _request: &TextRequest,
        ) -> Result<
            Box<dyn Stream<Item = Result<String, ProviderError>> + Send + Unpin>,
            ProviderError,
        > {
            Err(ProviderError::unsupported("stream_complete"))
        }
    }

    fn facts(input_tokens: u32, output_tokens: u32) -> StructuredResponse {
        StructuredResponse {
            value: serde_json::json!({ "title": "Requiem", "genre": "RPG" }),
            model: "scripted-model".into(),
            input_tokens,
            output_tokens,
        }
    }

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

    #[tokio::test(start_paused = true)]
    async fn a_refusal_is_returned_at_once_rather_than_asked_again() {
        let (provider, calls) = Scripted::new([
            Err(ProviderError::api(401, "invalid x-api-key")),
            Ok(facts(1, 1)),
        ]);
        let client = AiClient::new(Box::new(provider)).with_retries(3);

        let error = client
            .complete_structured::<Facts>("the facts", &schema(), "sys", "usr", 100)
            .await
            .unwrap_err();

        assert!(matches!(error, AiError::Provider(_)), "{error:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "a 401 was asked again");
    }

    #[tokio::test(start_paused = true)]
    async fn a_recoverable_failure_is_asked_again_after_a_backoff() {
        let (provider, calls) = Scripted::new([
            Err(ProviderError::api(503, "overloaded")),
            Err(ProviderError::network("connection reset")),
            Ok(facts(4, 6)),
        ]);
        let client = AiClient::new(Box::new(provider)).with_retries(2);
        let started = tokio::time::Instant::now();

        let facts: Facts = client
            .complete_structured("the facts", &schema(), "sys", "usr", 100)
            .await
            .unwrap();

        assert_eq!(facts.title, "Requiem");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(
            started.elapsed() >= Duration::from_millis(750 + 1500),
            "the retries did not back off: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn a_structured_exchange_records_its_model_and_every_token_it_spent() {
        let (provider, _) = Scripted::new([
            Ok(StructuredResponse {
                value: serde_json::json!({ "title": "only" }),
                ..facts(10, 20)
            }),
            Ok(facts(3, 4)),
        ]);
        let client = AiClient::new(Box::new(provider));

        let _: Facts = client
            .complete_structured("the facts", &schema(), "sys", "usr", 100)
            .await
            .unwrap();

        let log = client.exchanges();
        assert_eq!(log[0].model, "scripted-model");
        assert_eq!((log[0].input_tokens, log[0].output_tokens), (13, 24));
        assert_eq!(log[0].attempts, 2);
        assert_eq!(client.token_totals(), (13, 24));
    }

    #[test]
    fn debug_names_the_provider() {
        let rendered = format!("{:?}", AiClient::new(Box::new(MockProvider::new(MISSING))));
        assert!(rendered.contains("mock"), "{rendered}");
    }
}
