use super::Capacity;
use super::ChatError;
use super::deployment::Deployment;

/// Fallback allocation when a deployment has not reported native capacity.
/// Production callers supply validated typed configuration through
/// [`Resolver::with_context`].
pub const DEFAULT_CONTEXT: u64 = 32_768;

/// Finds the effective context limit of the deployment a model alias routes
/// to, from the deployment itself and never from the model's name.
///
/// Cold Ollama uses the advertised native window from `/api/show`. A loaded
/// `/api/ps` runtime still wins, and an explicit LiteLLM route `num_ctx` is an
/// operator request bounded by that native window. A configured context is a
/// fallback only when native capacity is unknown, never a ceiling on a larger
/// advertised window. The same `/api/show` response says whether the engine
/// thinks and whether it reads images.
///
/// LiteLLM 1.99.1 accepts top-level `num_ctx` and forwards it to Ollama's
/// `options.num_ctx`, so ordinary inference and summarization must both apply
/// the returned setting to this exact alias. See
/// <https://docs.litellm.ai/docs/proxy/model_management> and
/// <https://docs.ollama.com/api/ps>.
///
/// An endpoint that is not such a deployment is resolved with
/// [`Resolver::undisclosed`], which asks it nothing.
///
/// Refreshed per preparation: no stale cross-provider or unloaded-runtime cache.
pub struct Resolver {
    configured: Option<u64>,
    deployment: Option<Deployment>,
}

impl Resolver {
    /// A resolver falling back to [`DEFAULT_CONTEXT`] when a deployment
    /// reports no capacity.
    pub fn new(host: &str, key: &str, ollama: &str) -> Result<Self, ChatError> {
        Self::with_context(host, key, ollama, Some(DEFAULT_CONTEXT))
    }

    /// A resolver falling back to `configured` when a deployment reports no
    /// capacity, or to nothing when that is unset or zero.
    ///
    /// Fails only when no HTTP client can be built on this host, such as
    /// when its TLS backend cannot load.
    pub fn with_context(
        host: &str,
        key: &str,
        ollama: &str,
        configured: Option<u64>,
    ) -> Result<Self, ChatError> {
        Ok(Self {
            configured: configured.filter(|value| *value > 0),
            deployment: Some(Deployment::new(host, key, ollama)?),
        })
    }

    /// A resolver for an endpoint that is not a LiteLLM deployment of this
    /// instance.
    ///
    /// Nothing is requested to learn a model's capacity: every model is given
    /// `configured`, or an unknown capacity when that is unset or zero, and no
    /// Ollama `num_ctx` is ever set for it.
    pub fn undisclosed(configured: Option<u64>) -> Self {
        Self {
            configured: configured.filter(|value| *value > 0),
            deployment: None,
        }
    }

    /// The effective context capacity of the deployment `model` routes to,
    /// falling back to the proxy's wildcard route when `model` has none.
    ///
    /// Never fails: when metadata cannot be fetched or the route is
    /// ambiguous, the capacity's limit is unknown and its reason says why.
    pub async fn resolve(&self, model: &str) -> Capacity {
        match &self.deployment {
            Some(deployment) => deployment.resolve(model, self.configured).await,
            None => Capacity::assumed(model, self.configured),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_json;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    use super::super::Source;
    use super::*;

    async fn fixture(parameters: Value, running: Value, shown: Value) -> (MockServer, Resolver) {
        let server = MockServer::start().await;
        let mut parameters = parameters;
        parameters["api_base"] = json!(server.uri());
        Mock::given(method("GET"))
            .and(path("/v2/model/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"data":[{"model_name":"alias","litellm_params":parameters}]}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/ps"))
            .respond_with(ResponseTemplate::new(200).set_body_json(running))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .and(body_json(json!({"model":"native:latest"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(shown))
            .mount(&server)
            .await;
        let resolver =
            Resolver::with_context(&server.uri(), "key", &server.uri(), Some(DEFAULT_CONTEXT))
                .unwrap();
        (server, resolver)
    }

    #[tokio::test]
    async fn cold_model_uses_explicit_bounded_context_without_loaded_runtime() {
        let (_server,resolver) = fixture(json!({"model":"ollama_chat/native:latest"}), json!({"models":[]}),json!({"parameters":"temperature 0.7", "model_info":{"general.architecture":"qwen3","qwen3.context_length":16384}})).await;
        let capacity = resolver.resolve("alias").await;
        assert_eq!(capacity.limit, Some(16384));
        assert_eq!(capacity.ollama, Some(16384));
        assert_eq!(capacity.source, Source::Provider);
    }

    #[tokio::test]
    async fn cold_model_uses_advertised_native_window_instead_of_default_clamp() {
        let (_server, resolver) = fixture(
            json!({"model": "ollama_chat/native:latest"}),
            json!({"models": []}),
            json!({"model_info": {"general.architecture": "qwen3", "qwen3.context_length": 262144}}),
        )
        .await;
        let capacity = resolver.resolve("alias").await;
        assert_eq!(capacity.limit, Some(262144));
        assert_eq!(capacity.ollama, Some(262144));
        assert_eq!(capacity.source, Source::Provider);
    }

    #[tokio::test]
    async fn explicit_route_num_ctx_is_an_operator_request_bounded_by_native() {
        let (_server, resolver) = fixture(
            json!({"model": "ollama_chat/native:latest", "num_ctx": 8192}),
            json!({"models": []}),
            json!({"model_info": {"general.architecture": "qwen3", "qwen3.context_length": 65536}}),
        )
        .await;
        let capacity = resolver.resolve("alias").await;
        assert_eq!(capacity.limit, Some(8192));
        assert_eq!(capacity.ollama, Some(8192));
        assert_eq!(capacity.source, Source::Configured);
    }

    #[tokio::test]
    async fn explicit_route_num_ctx_never_exceeds_native() {
        let (_server, resolver) = fixture(
            json!({"model": "ollama_chat/native:latest", "num_ctx": 131072}),
            json!({"models": []}),
            json!({"model_info": {"general.architecture": "qwen3", "qwen3.context_length": 65536}}),
        )
        .await;
        let capacity = resolver.resolve("alias").await;
        assert_eq!(capacity.limit, Some(65536));
        assert_eq!(capacity.ollama, Some(65536));
        assert_eq!(capacity.source, Source::Configured);
        assert!(capacity.reason.is_some());
    }

    #[tokio::test]
    async fn show_parameter_num_ctx_does_not_shrink_advertised_native() {
        let (_server, resolver) = fixture(
            json!({"model": "ollama_chat/native:latest"}),
            json!({"models": []}),
            json!({
                "parameters": "temperature 0.7\nnum_ctx 4096",
                "model_info": {"general.architecture": "qwen3", "qwen3.context_length": 262144}
            }),
        )
        .await;
        let capacity = resolver.resolve("alias").await;
        assert_eq!(capacity.limit, Some(262144));
        assert_eq!(capacity.ollama, Some(262144));
        assert_eq!(capacity.source, Source::Provider);
    }

    #[tokio::test]
    async fn unknown_native_capacity_uses_configured_fallback() {
        let (_server, resolver) = fixture(
            json!({"model": "ollama_chat/native:latest"}),
            json!({"models": []}),
            json!({"parameters": "temperature 0.7"}),
        )
        .await;
        let capacity = resolver.resolve("alias").await;
        assert_eq!(capacity.limit, Some(DEFAULT_CONTEXT));
        assert_eq!(capacity.ollama, Some(DEFAULT_CONTEXT));
        assert_eq!(capacity.source, Source::Configured);
    }

    #[tokio::test]
    async fn unknown_native_capacity_without_fallback_stays_unknown() {
        let server = MockServer::start().await;
        let mut parameters = json!({"model": "ollama_chat/native:latest"});
        parameters["api_base"] = json!(server.uri());
        Mock::given(method("GET"))
            .and(path("/v2/model/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"data":[{"model_name":"alias","litellm_params":parameters}]}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/ps"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models":[]})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .and(body_json(json!({"model": "native:latest"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        let resolver = Resolver::with_context(&server.uri(), "key", &server.uri(), None).unwrap();
        let capacity = resolver.resolve("alias").await;
        assert_eq!(capacity.limit, None);
        assert_eq!(capacity.ollama, None);
        assert_eq!(capacity.source, Source::Unknown);
    }

    #[tokio::test]
    async fn exact_runtime_wins_over_training_and_unrelated_loaded_models() {
        let (_server,resolver) = fixture(json!({"model":"ollama_chat/native:latest","num_ctx":32768}), json!({"models":[{"name":"other","context_length":131072},{"name":"native:latest","context_length":8192}]}),json!({"model_info":{"general.architecture":"qwen3","qwen3.context_length":32768}})).await;
        let capacity = resolver.resolve("alias").await;
        assert_eq!(capacity.limit, Some(8192));
        assert_eq!(capacity.ollama, Some(8192));
        assert_eq!(capacity.source, Source::Runtime);
    }

    #[tokio::test]
    async fn provider_alias_never_inherits_ollama_metadata_or_options() {
        let server = MockServer::start().await;
        Mock::given(path("/v2/model/info")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{"model_name":"alias","litellm_params":{"model":"openai/remote"},"model_info":{"max_input_tokens":128000}}]}))).mount(&server).await;
        let resolver = Resolver::new(&server.uri(), "key", &server.uri()).unwrap();
        let capacity = resolver.resolve("alias").await;
        assert_eq!(capacity.limit, Some(128000));
        assert_eq!(capacity.ollama, None);
        assert_eq!(capacity.source, Source::Provider);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_provider_route_reports_the_vision_its_model_info_declares() {
        for (declared, vision) in [
            (
                json!({"max_input_tokens": 128000, "supports_vision": true}),
                Some(true),
            ),
            (
                json!({"max_input_tokens": 128000, "supports_vision": false}),
                Some(false),
            ),
            (json!({"max_input_tokens": 128000}), None),
        ] {
            let server = MockServer::start().await;
            Mock::given(path("/v2/model/info"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{"model_name":"alias","litellm_params":{"model":"openai/remote"},"model_info":declared}]})))
                .mount(&server)
                .await;
            let resolver = Resolver::new(&server.uri(), "key", &server.uri()).unwrap();
            assert_eq!(resolver.resolve("alias").await.vision, vision, "{declared}");
        }
    }

    #[tokio::test]
    async fn provider_changes_invalidate_runtime_observation_immediately() {
        let (server, resolver) = fixture(
            json!({"model":"ollama_chat/native:latest"}),
            json!({"models":[{"name":"native:latest","context_length":8192}]}),
            json!({}),
        )
        .await;
        assert_eq!(resolver.resolve("alias").await.limit, Some(8192));
        server.reset().await;
        Mock::given(path("/v2/model/info")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{"model_name":"alias","litellm_params":{"model":"openai/remote"},"model_info":{"max_input_tokens":64000}}]}))).mount(&server).await;
        let capacity = resolver.resolve("alias").await;
        assert_eq!(capacity.limit, Some(64000));
        assert_eq!(capacity.ollama, None);
    }

    #[tokio::test]
    async fn wildcard_metadata_resolves_cold_nonpreset_model_without_v1_example_substitution() {
        use wiremock::matchers::query_param;
        let server = MockServer::start().await;
        Mock::given(path("/v2/model/info"))
            .and(query_param("model", "custom:1b"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"data":[],"total_pages":0})),
            )
            .mount(&server)
            .await;
        Mock::given(path("/v2/model/info")).and(query_param("model","*"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{"model_name":"*","litellm_params":{"model":"ollama_chat/*","api_base":server.uri()}}],"total_pages":1}))).mount(&server).await;
        Mock::given(path("/api/ps"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models":[]})))
            .mount(&server)
            .await;
        Mock::given(path("/api/show")).and(body_json(json!({"model":"custom:1b"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"model_info":{"general.architecture":"custom","custom.context_length":65536}}))).mount(&server).await;
        let resolver =
            Resolver::with_context(&server.uri(), "key", &server.uri(), Some(32768)).unwrap();
        let capacity = resolver.resolve("custom:1b").await;
        assert_eq!(capacity.limit, Some(65536));
        assert_eq!(capacity.source, Source::Provider);
        assert_eq!(capacity.ollama, Some(65536));
        assert!(capacity.identity.contains("ollama_chat/custom:1b"));
        assert_eq!(server.received_requests().await.unwrap().len(), 4);
    }

    #[tokio::test]
    async fn ollama_thinking_capability_enables_reasoning() {
        let (_server, resolver) = fixture(
            json!({"model":"ollama_chat/native:latest"}),
            json!({"models":[]}),
            json!({
                "capabilities": ["completion", "thinking"],
                "model_info": {"general.architecture":"qwen3","qwen3.context_length":16384}
            }),
        )
        .await;
        assert!(resolver.resolve("alias").await.reasoning);
    }

    #[tokio::test]
    async fn provider_supports_reasoning_without_guessing_the_name() {
        let server = MockServer::start().await;
        Mock::given(path("/v2/model/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data":[{
                    "model_name":"alias",
                    "litellm_params":{"model":"anthropic/claude-sonnet"},
                    "model_info":{"max_input_tokens":200000,"supports_reasoning":true}
                }]
            })))
            .mount(&server)
            .await;
        let resolver = Resolver::new(&server.uri(), "key", &server.uri()).unwrap();
        let capacity = resolver.resolve("alias").await;
        assert!(capacity.reasoning);
        assert_eq!(capacity.ollama, None);
    }

    #[tokio::test]
    async fn an_undisclosed_endpoint_is_given_the_configured_context_without_a_num_ctx() {
        let capacity = Resolver::undisclosed(Some(DEFAULT_CONTEXT))
            .resolve("gpt-4o")
            .await;

        assert_eq!(capacity.limit, Some(DEFAULT_CONTEXT));
        assert_eq!(capacity.source, Source::Configured);
        assert_eq!(capacity.ollama, None, "no num_ctx may reach a third party");
        assert!(!capacity.reasoning);
        assert_eq!(capacity.vision, None);
        assert_eq!(capacity.identity, "gpt-4o");
        assert!(capacity.reason.is_some());
    }

    #[tokio::test]
    async fn an_undisclosed_endpoint_without_a_configured_context_is_unknown() {
        for configured in [None, Some(0)] {
            let capacity = Resolver::undisclosed(configured).resolve("gpt-4o").await;

            assert_eq!(capacity.source, Source::Unknown, "{configured:?}");
            assert_eq!(capacity.limit, None, "{configured:?}");
            assert_eq!(capacity.ollama, None, "{configured:?}");
            assert!(capacity.reason.is_some(), "{configured:?}");
        }
    }

    #[tokio::test]
    async fn ollama_vision_capability_is_read_from_the_same_show_response() {
        let (server, resolver) = fixture(
            json!({"model":"ollama_chat/native:latest"}),
            json!({"models":[]}),
            json!({
                "capabilities": ["completion", "vision"],
                "model_info": {"general.architecture":"gemma3","gemma3.context_length":16384}
            }),
        )
        .await;
        assert_eq!(resolver.resolve("alias").await.vision, Some(true));
        let shows = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|request| request.url.path() == "/api/show")
            .count();
        assert_eq!(shows, 1, "Vision must not cost a second /api/show");
    }

    #[tokio::test]
    async fn a_text_only_ollama_model_reports_no_vision() {
        let (_server, resolver) = fixture(
            json!({"model":"ollama_chat/native:latest"}),
            json!({"models":[]}),
            json!({
                "capabilities": ["completion", "tools"],
                "model_info": {"general.architecture":"qwen3","qwen3.context_length":16384}
            }),
        )
        .await;
        assert_eq!(resolver.resolve("alias").await.vision, Some(false));
    }

    #[tokio::test]
    async fn a_loaded_runtime_still_reports_vision() {
        let (_server, resolver) = fixture(
            json!({"model":"ollama_chat/native:latest"}),
            json!({"models":[{"name":"native:latest","context_length":8192}]}),
            json!({"capabilities": ["completion", "vision"]}),
        )
        .await;
        let capacity = resolver.resolve("alias").await;
        assert_eq!(capacity.source, Source::Runtime);
        assert_eq!(capacity.vision, Some(true));
    }

    #[tokio::test]
    async fn vision_is_unknown_without_capabilities() {
        let (_server, resolver) = fixture(
            json!({"model":"ollama_chat/native:latest"}),
            json!({"models":[]}),
            json!({"model_info": {"general.architecture":"qwen3","qwen3.context_length":16384}}),
        )
        .await;
        assert_eq!(resolver.resolve("alias").await.vision, None);
    }

    #[tokio::test]
    async fn vision_is_unknown_for_a_provider_endpoint() {
        let server = MockServer::start().await;
        Mock::given(path("/v2/model/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data":[{
                    "model_name":"alias",
                    "litellm_params":{"model":"openai/remote"},
                    "model_info":{"max_input_tokens":128000}
                }]
            })))
            .mount(&server)
            .await;
        let resolver = Resolver::new(&server.uri(), "key", &server.uri()).unwrap();
        assert_eq!(resolver.resolve("alias").await.vision, None);
    }

    #[tokio::test]
    async fn the_metadata_request_carries_the_key_as_a_bearer_token() {
        let server = MockServer::start().await;
        Mock::given(path("/v2/model/info"))
            .and(header("authorization", "Bearer key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data":[{
                    "model_name":"alias",
                    "litellm_params":{"model":"openai/remote"},
                    "model_info":{"max_input_tokens":128000}
                }]
            })))
            .mount(&server)
            .await;
        let resolver = Resolver::new(&server.uri(), "key", &server.uri()).unwrap();
        assert_eq!(resolver.resolve("alias").await.limit, Some(128000));
    }
}
