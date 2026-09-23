//! One handle in front of many providers.

use std::borrow::Cow;
use std::sync::Arc;

use async_trait::async_trait;

use crate::provider::capabilities::Capabilities;
use crate::provider::completion::Completion;
use crate::provider::completion_provider::CompletionProvider;
use crate::provider::error::ProviderError;
use crate::provider::kind::ProviderKind;
use crate::provider::request::CompletionRequest;
use crate::provider::strategy::SelectionStrategy;
use crate::provider::weighted::Weighted;

const DEFAULT_NAME: &str = "router";

/// Routes a completion to one of several providers.
///
/// A [`Router`] is itself a [`CompletionProvider`], so a consumer holds one
/// handle and never learns whether it is talking to a single model, an A/B
/// split, or a chain three deep. Routers compose for the same reason: a
/// weighted split between two chains is a router of routers.
#[derive(Debug, Clone)]
pub struct Router {
    providers: Vec<Weighted>,
    strategy: SelectionStrategy,
    name: String,
    experiment: Option<String>,
    required: Capabilities,
}

impl Router {
    pub fn new(providers: Vec<Weighted>, strategy: SelectionStrategy) -> Self {
        Self {
            providers,
            strategy,
            name: DEFAULT_NAME.to_string(),
            experiment: None,
            required: Capabilities::NONE,
        }
    }

    /// A single provider, used for every request.
    pub fn primary(provider: Arc<dyn CompletionProvider>) -> Self {
        Self::new(vec![Weighted::spare(provider)], SelectionStrategy::Primary)
    }

    /// Providers tried in order until one answers.
    pub fn fallback(providers: Vec<Arc<dyn CompletionProvider>>) -> Self {
        Self::new(
            providers.into_iter().map(Weighted::spare).collect(),
            SelectionStrategy::Fallback,
        )
    }

    /// An A/B split: each request goes to one arm drawn by weight, and a
    /// failure is that arm's failure.
    pub fn weighted(providers: Vec<Weighted>) -> Self {
        Self::new(providers, SelectionStrategy::Weighted)
    }

    /// An A/B split, with the arms not drawn standing by as backups.
    pub fn weighted_fallback(providers: Vec<Weighted>) -> Self {
        Self::new(providers, SelectionStrategy::WeightedFallback)
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Label the split this router serves, so the arm that answered a request
    /// can be attributed to the experiment that asked for it.
    pub fn with_experiment(mut self, experiment: impl Into<String>) -> Self {
        self.experiment = Some(experiment.into());
        self
    }

    /// Route only to providers that support all of these.
    ///
    /// A request that needs a schema-constrained answer or a cost line has no
    /// business reaching a provider that cannot produce one, and silently
    /// dropping the requirement is worse than saying no.
    pub fn requiring(mut self, required: Capabilities) -> Self {
        self.required = required;
        self
    }

    pub fn strategy(&self) -> SelectionStrategy {
        self.strategy
    }

    pub fn providers(&self) -> &[Weighted] {
        &self.providers
    }

    pub fn experiment(&self) -> Option<&str> {
        self.experiment.as_deref()
    }

    pub fn required(&self) -> Capabilities {
        self.required
    }

    /// Run a request against a caller-supplied split sample.
    ///
    /// [`CompletionProvider::complete`] draws the sample from entropy. Passing
    /// it in keeps the routing decision reproducible for a test, and lets a
    /// caller pin an experiment bucket to something stable such as a task id.
    pub async fn complete_with_sample(
        &self,
        request: CompletionRequest<'_>,
        sample: f64,
    ) -> Result<Completion, ProviderError> {
        if self.providers.is_empty() {
            return Err(ProviderError::Unconfigured);
        }

        let candidates = self.candidates();
        if candidates.is_empty() {
            return Err(ProviderError::unsupported_route(&self.name));
        }

        let start = self
            .strategy
            .start(&candidates, sample)
            .min(candidates.len() - 1);

        if !self.strategy.chains() {
            return candidates[start].provider.complete(request).await;
        }

        let mut attempted = 0;
        let mut last: Option<ProviderError> = None;

        for index in self.order(candidates.len(), start) {
            let provider = &candidates[index].provider;
            attempted += 1;

            match provider.complete(request).await {
                // A completion the model produced is an answer even when the
                // caller does not like it, so a chain stops here.
                Ok(completion) => return Ok(completion),
                Err(error) => {
                    // A request the provider refused on its merits will be
                    // refused identically by the next one, and retrying it
                    // only spends another provider's budget.
                    if !error.recoverable() {
                        return Err(error);
                    }
                    tracing::warn!(
                        router = %self.name,
                        experiment = self.experiment.as_deref().unwrap_or("none"),
                        provider = provider.name(),
                        error = %error,
                        "provider failed, trying the next"
                    );
                    last = Some(error);
                }
            }
        }

        Err(match last {
            // Reporting the last provider's own words is the whole point: the
            // caller decides whether to retry by reading them.
            Some(last) => ProviderError::Exhausted {
                attempted,
                last: Box::new(last),
            },
            None => ProviderError::Unconfigured,
        })
    }

    /// The providers eligible to serve this router's requests.
    fn candidates(&self) -> Cow<'_, [Weighted]> {
        if self.required == Capabilities::NONE {
            return Cow::Borrowed(&self.providers);
        }
        Cow::Owned(
            self.providers
                .iter()
                .filter(|weighted| weighted.provider.capabilities().satisfies(self.required))
                .cloned()
                .collect(),
        )
    }

    /// The candidate indices to try, starting where the strategy pointed.
    fn order(&self, len: usize, start: usize) -> Vec<usize> {
        let mut order = Vec::with_capacity(len);
        order.push(start);
        order.extend((0..len).filter(|index| *index != start));
        order
    }

    /// The candidates any request can actually reach under this strategy.
    ///
    /// A primary router only ever asks its first candidate, and a weighted
    /// split without fallback never draws an arm of zero weight unless no arm
    /// has any; every other arm is reachable.
    fn reachable(&self) -> Vec<Weighted> {
        let candidates = self.candidates();
        match self.strategy {
            SelectionStrategy::Primary => candidates.iter().take(1).cloned().collect(),
            SelectionStrategy::Weighted => {
                let drawn: Vec<Weighted> = candidates
                    .iter()
                    .filter(|weighted| weighted.weight.is_finite() && weighted.weight > 0.0)
                    .cloned()
                    .collect();
                if drawn.is_empty() {
                    candidates.iter().take(1).cloned().collect()
                } else {
                    drawn
                }
            }
            SelectionStrategy::Fallback | SelectionStrategy::WeightedFallback => {
                candidates.into_owned()
            }
        }
    }
}

#[async_trait]
impl CompletionProvider for Router {
    fn name(&self) -> &str {
        &self.name
    }

    /// The kind every arm a request can reach shares, or
    /// [`ProviderKind::Mixed`] when they differ. A router with no reachable
    /// arm reports [`ProviderKind::Http`].
    fn kind(&self) -> ProviderKind {
        let mut kinds = self
            .reachable()
            .into_iter()
            .map(|weighted| weighted.provider.kind());
        let Some(first) = kinds.next() else {
            return ProviderKind::Http;
        };
        if kinds.all(|kind| kind == first) {
            first
        } else {
            ProviderKind::Mixed
        }
    }

    /// What every arm a request can reach supports.
    ///
    /// Reporting any single arm's capabilities would let an outer router's
    /// requirement pass here and then land on an arm that cannot meet it, so a
    /// router promises only what all of its reachable arms share. A router with
    /// no reachable arm supports nothing.
    fn capabilities(&self) -> Capabilities {
        self.reachable()
            .into_iter()
            .map(|weighted| weighted.provider.capabilities())
            .reduce(Capabilities::intersection)
            .unwrap_or(Capabilities::NONE)
    }

    async fn complete(&self, request: CompletionRequest<'_>) -> Result<Completion, ProviderError> {
        self.complete_with_sample(request, rand::random_range(0.0..1.0))
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::Router;
    use crate::client::RequestOptions;
    use crate::provider::capabilities::Capabilities;
    use crate::provider::completion::Completion;
    use crate::provider::completion_provider::CompletionProvider;
    use crate::provider::error::ProviderError;
    use crate::provider::kind::ProviderKind;
    use crate::provider::request::CompletionRequest;
    use crate::provider::strategy::SelectionStrategy;
    use crate::provider::testing::StubProvider;
    use crate::provider::weighted::Weighted;
    use crate::wire::Message;
    use crate::wire::ToolDefinition;
    use crate::wire::Usage;

    const RESERVED: u32 = 512;

    fn request(messages: &[Message]) -> CompletionRequest<'_> {
        CompletionRequest::new(
            "test-model",
            messages,
            RequestOptions { reserved: RESERVED },
        )
    }

    async fn answer(router: &Router, sample: f64) -> Result<Completion, ProviderError> {
        let messages = [Message::user("hello")];
        router
            .complete_with_sample(request(&messages), sample)
            .await
    }

    fn structured() -> Capabilities {
        Capabilities {
            structured_output: true,
            ..Capabilities::NONE
        }
    }

    #[tokio::test]
    async fn a_primary_router_uses_its_only_provider() {
        let router = Router::primary(StubProvider::answering("local", "from local").shared());

        let completion = answer(&router, 0.9).await.expect("an answer");

        assert_eq!(completion.provider, "local");
        assert_eq!(completion.message.content.as_deref(), Some("from local"));
    }

    #[tokio::test]
    async fn a_primary_router_never_reaches_past_its_primary() {
        let backup = Arc::new(StubProvider::answering("backup", "from backup"));
        let router = Router::new(
            vec![
                Weighted::spare(StubProvider::failing("primary", "upstream reset").shared()),
                Weighted::spare(backup.clone()),
            ],
            SelectionStrategy::Primary,
        );

        let error = answer(&router, 0.5).await.expect_err("a failure");

        assert_eq!(backup.calls(), 0, "the backup was reached");
        assert!(
            matches!(&error, ProviderError::Agent { provider, message } if provider == "primary" && message == "upstream reset"),
            "a lone failure is reported as itself: {error:?}"
        );
    }

    #[tokio::test]
    async fn a_primary_router_answers_from_the_first_provider() {
        let second = Arc::new(StubProvider::answering("beta", "from beta"));
        let router = Router::new(
            vec![
                Weighted::spare(StubProvider::answering("alpha", "from alpha").shared()),
                Weighted::spare(second.clone()),
            ],
            SelectionStrategy::Primary,
        );

        let completion = answer(&router, 0.99).await.expect("an answer");

        assert_eq!(completion.provider, "alpha");
        assert_eq!(second.calls(), 0);
    }

    #[tokio::test]
    async fn a_weighted_split_honours_its_weights_across_a_seeded_sweep() {
        let router = Router::new(
            vec![
                Weighted::new(StubProvider::answering("control", "c").shared(), 80.0),
                Weighted::new(StubProvider::answering("variant", "v").shared(), 20.0),
            ],
            SelectionStrategy::Weighted,
        );

        let mut control = 0;
        let mut variant = 0;
        for step in 0..100 {
            let completion = answer(&router, f64::from(step) / 100.0)
                .await
                .expect("an answer");
            match completion.provider.as_str() {
                "control" => control += 1,
                "variant" => variant += 1,
                other => panic!("unexpected arm {other}"),
            }
        }

        assert_eq!((control, variant), (80, 20));
    }

    #[tokio::test]
    async fn a_weighted_split_reports_the_failure_of_the_arm_it_drew() {
        let router = Router::new(
            vec![
                Weighted::new(
                    StubProvider::failing("control", "upstream reset").shared(),
                    1.0,
                ),
                Weighted::new(StubProvider::answering("variant", "v").shared(), 0.0),
            ],
            SelectionStrategy::Weighted,
        );

        let error = answer(&router, 0.5).await.expect_err("a failure");

        assert!(
            matches!(error, ProviderError::Agent { .. }),
            "a split that does not chain reports its arm's own failure: {error:?}"
        );
        assert_eq!(error.provider(), Some("control"));
    }

    #[tokio::test]
    async fn a_chain_advances_past_a_failing_provider_and_stops_at_the_first_success() {
        let third = Arc::new(StubProvider::answering("third", "never reached"));
        let router = Router::fallback(vec![
            StubProvider::failing("first", "connection reset").shared(),
            StubProvider::answering("second", "from second").shared(),
            third.clone(),
        ]);

        let completion = answer(&router, 0.0).await.expect("an answer");

        assert_eq!(completion.provider, "second");
        assert_eq!(completion.message.content.as_deref(), Some("from second"));
        assert_eq!(third.calls(), 0, "the chain kept going after a success");
    }

    #[tokio::test]
    async fn a_chain_of_one_answers_from_that_one() {
        let router = Router::fallback(vec![StubProvider::answering("only", "from only").shared()]);

        let completion = answer(&router, 0.0).await.expect("an answer");

        assert_eq!(completion.provider, "only");
    }

    #[tokio::test]
    async fn a_chain_walks_past_every_failing_provider_before_it_answers() {
        let router = Router::fallback(vec![
            StubProvider::failing("first", "connection reset").shared(),
            StubProvider::failing("second", "socket hang up").shared(),
            StubProvider::answering("third", "from third").shared(),
        ]);

        let completion = answer(&router, 0.0).await.expect("an answer");

        assert_eq!(completion.provider, "third");
    }

    #[tokio::test]
    async fn an_empty_answer_is_still_an_answer_and_stops_the_chain() {
        let backup = Arc::new(StubProvider::answering("backup", "from backup"));
        let router = Router::fallback(vec![
            StubProvider::answering("first", "").shared(),
            backup.clone(),
        ]);

        let completion = answer(&router, 0.0).await.expect("an answer");

        assert_eq!(completion.provider, "first");
        assert_eq!(completion.message.content.as_deref(), Some(""));
        assert_eq!(backup.calls(), 0, "an answer was treated as a failure");
    }

    #[tokio::test]
    async fn an_exhausted_chain_reports_the_last_failure_not_a_generic_one() {
        let router = Router::fallback(vec![
            StubProvider::failing("first", "connection reset").shared(),
            StubProvider::failing("second", "socket hang up").shared(),
            StubProvider::failing("third", "429 rate limit reached").shared(),
        ]);

        let error = answer(&router, 0.0).await.expect_err("a failure");

        let ProviderError::Exhausted { attempted, last } = &error else {
            panic!("expected an exhausted chain, got {error:?}");
        };
        assert_eq!(*attempted, 3);
        assert_eq!(last.provider(), Some("third"));

        let rendered = error.to_string();
        assert!(
            rendered.contains("rate limit"),
            "lost the cause: {rendered}"
        );
        assert!(
            !rendered.contains("connection reset"),
            "reported an earlier failure: {rendered}"
        );
    }

    #[tokio::test]
    async fn a_weighted_chain_falls_back_from_the_arm_it_drew() {
        let router = Router::weighted_fallback(vec![
            Weighted::new(
                StubProvider::answering("control", "from control").shared(),
                50.0,
            ),
            Weighted::new(
                StubProvider::failing("variant", "upstream reset").shared(),
                50.0,
            ),
        ]);

        let drawn = answer(&router, 0.75).await.expect("an answer");

        assert_eq!(
            drawn.provider, "control",
            "the variant should have fallen back"
        );
    }

    #[tokio::test]
    async fn a_rejected_request_stops_the_chain_immediately() {
        let backup = Arc::new(StubProvider::answering("backup", "from backup"));
        let router = Router::fallback(vec![
            StubProvider::rejecting("first", "messages is malformed").shared(),
            backup.clone(),
        ]);

        let error = answer(&router, 0.0).await.expect_err("a failure");

        assert_eq!(backup.calls(), 0, "a bad request was retried elsewhere");
        assert!(matches!(error, ProviderError::Http { .. }));
    }

    #[tokio::test]
    async fn a_router_with_no_providers_says_so() {
        for strategy in [
            SelectionStrategy::Primary,
            SelectionStrategy::Weighted,
            SelectionStrategy::Fallback,
            SelectionStrategy::WeightedFallback,
        ] {
            let router = Router::new(Vec::new(), strategy);

            let error = answer(&router, 0.5).await.expect_err("a failure");

            assert!(
                matches!(error, ProviderError::Unconfigured),
                "{strategy:?} reported {error:?}"
            );
        }
    }

    #[tokio::test]
    async fn routers_compose_because_a_router_is_a_provider() {
        let inner = Router::fallback(vec![
            StubProvider::failing("inner-first", "connection reset").shared(),
            StubProvider::answering("inner-second", "from inner").shared(),
        ])
        .with_name("inner");

        let outer = Router::fallback(vec![
            StubProvider::failing("outer-first", "connection reset").shared(),
            Arc::new(inner),
        ]);

        let completion = answer(&outer, 0.0).await.expect("an answer");

        assert_eq!(completion.provider, "inner-second");
    }

    #[tokio::test]
    async fn the_drawn_arm_is_tried_before_the_rest_of_the_chain() {
        let control = Arc::new(StubProvider::answering("control", "c"));
        let router = Router::new(
            vec![
                Weighted::new(control.clone(), 50.0),
                Weighted::new(StubProvider::answering("variant", "v").shared(), 50.0),
            ],
            SelectionStrategy::WeightedFallback,
        );

        let completion = answer(&router, 0.9).await.expect("an answer");

        assert_eq!(completion.provider, "variant");
        assert_eq!(control.calls(), 0, "the unchosen arm was tried first");
    }

    #[tokio::test]
    async fn the_request_reaches_the_provider_exactly_as_it_was_built() {
        let provider = Arc::new(StubProvider::answering("only", "ok"));
        let router = Router::primary(provider.clone());
        let messages = [Message::system("be brief"), Message::user("hello")];
        let tools = [ToolDefinition::function(
            "read_file",
            "Read a file",
            serde_json::json!({"type": "object"}),
        )];

        router
            .complete_with_sample(
                CompletionRequest::new("qwen3", &messages, RequestOptions { reserved: 4096 })
                    .with_tools(&tools),
                0.0,
            )
            .await
            .expect("an answer");

        let seen = provider.seen().expect("the provider was called");
        assert_eq!(seen.model, "qwen3");
        assert_eq!(seen.messages.len(), 2);
        assert_eq!(seen.messages[1].content.as_deref(), Some("hello"));
        assert_eq!(seen.tools.expect("tools")[0].function.name, "read_file");
        assert_eq!(seen.options.reserved, 4096);
    }

    #[tokio::test]
    async fn the_completion_the_answering_arm_built_is_the_one_the_caller_gets() {
        let router = Router::fallback(vec![
            StubProvider::failing("first", "connection reset").shared(),
            StubProvider::answering("second", "from second")
                .with_usage(Usage::new(11, 22))
                .shared(),
        ]);

        let completion = answer(&router, 0.0).await.expect("an answer");

        assert_eq!(completion.provider, "second");
        assert_eq!(completion.finish_reason.as_deref(), Some("stop"));
        assert_eq!(completion.usage.expect("usage").total_tokens, 33);
    }

    #[test]
    fn a_router_names_itself_until_it_is_told_otherwise() {
        let router = Router::primary(StubProvider::answering("local", "hi").shared());

        assert_eq!(router.name(), "router");
        assert_eq!(router.with_name("split").name(), "split");
    }

    #[test]
    fn a_router_carries_the_experiment_its_split_belongs_to() {
        let router = Router::weighted(vec![Weighted::new(
            StubProvider::answering("control", "c").shared(),
            1.0,
        )]);

        assert_eq!(router.experiment(), None);
        assert_eq!(
            router.with_experiment("prompt-v2").experiment(),
            Some("prompt-v2")
        );
    }

    #[test]
    fn a_router_reads_its_strategy_out_of_configuration() {
        for (configured, expected) in [
            ("weighted_random", SelectionStrategy::Weighted),
            ("fallback", SelectionStrategy::Fallback),
            ("weighted_fallback", SelectionStrategy::WeightedFallback),
            ("nonsense", SelectionStrategy::Primary),
        ] {
            let router = Router::new(
                vec![Weighted::spare(
                    StubProvider::answering("local", "hi").shared(),
                )],
                configured.parse().unwrap_or_default(),
            );

            assert_eq!(router.strategy(), expected, "{configured}");
        }
    }

    #[test]
    fn a_chain_supports_only_what_every_arm_supports() {
        let router = Router::new(
            vec![
                Weighted::spare(
                    StubProvider::answering("first", "hi")
                        .with_capabilities(Capabilities::ALL)
                        .shared(),
                ),
                Weighted::spare(
                    StubProvider::answering("second", "hi")
                        .with_capabilities(structured())
                        .shared(),
                ),
            ],
            SelectionStrategy::Fallback,
        );

        assert_eq!(router.kind(), ProviderKind::Http);
        assert_eq!(router.capabilities(), structured());
        assert_eq!(router.providers().len(), 2);
    }

    #[test]
    fn a_primary_router_supports_what_its_only_reachable_arm_supports() {
        let router = Router::new(
            vec![
                Weighted::spare(
                    StubProvider::answering("first", "hi")
                        .with_capabilities(Capabilities::ALL)
                        .with_kind(ProviderKind::Cli)
                        .shared(),
                ),
                Weighted::spare(StubProvider::answering("second", "hi").shared()),
            ],
            SelectionStrategy::Primary,
        );

        assert_eq!(router.capabilities(), Capabilities::ALL);
        assert_eq!(router.kind(), ProviderKind::Cli);
    }

    #[test]
    fn a_split_ignores_the_arms_it_can_never_draw() {
        let router = Router::weighted(vec![
            Weighted::new(
                StubProvider::answering("drawn", "hi")
                    .with_capabilities(structured())
                    .shared(),
                1.0,
            ),
            Weighted::spare(StubProvider::answering("spare", "hi").shared()),
        ]);

        assert_eq!(router.capabilities(), structured());
    }

    #[test]
    fn a_router_over_both_kinds_says_it_is_mixed() {
        let router = Router::fallback(vec![
            StubProvider::answering("gateway", "hi").shared(),
            StubProvider::answering("agent", "hi")
                .with_kind(ProviderKind::Cli)
                .shared(),
        ]);

        assert_eq!(router.kind(), ProviderKind::Mixed);
    }

    #[tokio::test]
    async fn an_outer_requirement_never_reaches_an_incapable_arm_of_a_nested_split() {
        const REQUESTS: usize = 64;
        let incapable = Arc::new(StubProvider::answering("incapable", "plain"));
        let inner = Router::weighted(vec![
            Weighted::new(
                StubProvider::answering("capable", "structured")
                    .with_capabilities(structured())
                    .shared(),
                50.0,
            ),
            Weighted::new(incapable.clone(), 50.0),
        ])
        .with_name("inner");
        let outer = Router::fallback(vec![
            Arc::new(inner),
            StubProvider::answering("direct", "structured")
                .with_capabilities(structured())
                .shared(),
        ])
        .requiring(structured());

        for step in 0..REQUESTS {
            answer(&outer, step as f64 / REQUESTS as f64)
                .await
                .expect("an answer");
        }

        assert_eq!(
            incapable.calls(),
            0,
            "a requirement leaked through a nested split"
        );
    }

    #[test]
    fn an_empty_router_supports_nothing() {
        let router = Router::new(Vec::new(), SelectionStrategy::Fallback);

        assert_eq!(router.capabilities(), Capabilities::NONE);
        assert_eq!(router.kind(), ProviderKind::Http);
        assert!(router.providers().is_empty());
    }

    #[tokio::test]
    async fn a_requirement_routes_around_the_providers_that_cannot_meet_it() {
        let incapable = Arc::new(StubProvider::answering("plain", "from plain"));
        let router = Router::new(
            vec![
                Weighted::spare(incapable.clone()),
                Weighted::spare(
                    StubProvider::answering("structured", "from structured")
                        .with_capabilities(structured())
                        .shared(),
                ),
            ],
            SelectionStrategy::Fallback,
        )
        .requiring(structured());

        let completion = answer(&router, 0.0).await.expect("an answer");

        assert_eq!(completion.provider, "structured");
        assert_eq!(incapable.calls(), 0, "an incapable provider was asked");
        assert_eq!(router.required(), structured());
    }

    #[tokio::test]
    async fn a_requirement_no_provider_meets_is_refused_rather_than_dropped() {
        let router = Router::fallback(vec![
            StubProvider::answering("plain", "from plain").shared(),
            StubProvider::answering("also-plain", "from also-plain").shared(),
        ])
        .with_name("strict")
        .requiring(Capabilities::ALL);

        let error = answer(&router, 0.5).await.expect_err("a failure");

        assert!(
            matches!(&error, ProviderError::Unsupported { detail } if detail.starts_with("strict")),
            "expected a capability refusal, got {error:?}"
        );
        assert!(!error.recoverable());
    }

    #[tokio::test]
    async fn a_requirement_narrows_the_weighted_split_to_the_eligible_arms() {
        let router = Router::new(
            vec![
                Weighted::new(StubProvider::answering("plain", "p").shared(), 90.0),
                Weighted::new(
                    StubProvider::answering("structured", "s")
                        .with_capabilities(structured())
                        .shared(),
                    10.0,
                ),
            ],
            SelectionStrategy::Weighted,
        )
        .requiring(structured());

        for step in 0..20 {
            let completion = answer(&router, f64::from(step) / 20.0)
                .await
                .expect("an answer");
            assert_eq!(completion.provider, "structured");
        }
    }
}
