use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

use super::route::Route;
use super::routes::Routes;
use super::{Capacity, Error, Source};

/// Fallback allocation when a deployment has not reported native capacity.
/// Production callers supply validated typed configuration through
/// [`Resolver::with_context`].
pub const DEFAULT_CONTEXT: u64 = 32_768;

/// How long a metadata request may take before capacity is reported unknown.
const METADATA_TIMEOUT: Duration = Duration::from_secs(2);

/// Finds the effective context limit of the deployment a model alias routes
/// to, from the deployment itself and never from the model's name.
///
/// Cold Ollama uses the advertised native window from `/api/show`. A loaded
/// `/api/ps` runtime still wins, and an explicit LiteLLM route `num_ctx` is an
/// operator request bounded by that native window. A configured context is a
/// fallback only when native capacity is unknown, never a ceiling on a larger
/// advertised window.
///
/// LiteLLM 1.99.1 accepts top-level `num_ctx` and forwards it to Ollama's
/// `options.num_ctx`, so ordinary inference and summarization must both apply
/// the returned setting to this exact alias. See
/// <https://docs.litellm.ai/docs/proxy/model_management> and
/// <https://docs.ollama.com/api/ps>.
///
/// Refreshed per preparation: no stale cross-provider or unloaded-runtime cache.
pub struct Resolver {
    client: Client,
    host: String,
    key: String,
    ollama: String,
    configured: Option<u64>,
}

impl Resolver {
    /// A resolver falling back to [`DEFAULT_CONTEXT`] when a deployment
    /// reports no capacity.
    pub fn new(host: &str, key: &str, ollama: &str) -> Result<Self, Error> {
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
    ) -> Result<Self, Error> {
        let client = Client::builder()
            .connect_timeout(METADATA_TIMEOUT)
            .timeout(METADATA_TIMEOUT)
            .build()?;
        Ok(Self {
            client,
            host: host.trim_end_matches('/').into(),
            key: key.into(),
            ollama: ollama.trim_end_matches('/').into(),
            configured: configured.filter(|value| *value > 0),
        })
    }

    pub async fn resolve(&self, model: &str) -> Capacity {
        let Some(mut routes) = self.routes(model).await else {
            return Capacity::unknown(
                model,
                "The selected model's deployment metadata is unavailable.",
            );
        };
        if routes.is_empty() {
            let Some(wildcard) = self.routes("*").await else {
                return Capacity::unknown(
                    model,
                    "The wildcard deployment metadata is unavailable.",
                );
            };
            routes = wildcard;
        }
        let route = match select(&routes, model) {
            Ok(route) => route,
            Err(reason) => return Capacity::unknown(model, reason),
        };
        let identity = format!(
            "{}|{}|{}",
            model,
            route.litellm_params.model,
            route.litellm_params.api_base.as_deref().unwrap_or("")
        );
        let native = route
            .litellm_params
            .model
            .strip_prefix("ollama_chat/")
            .or_else(|| route.litellm_params.model.strip_prefix("ollama/"));
        let Some(native) = native else {
            let limit = route
                .model_info
                .get("max_input_tokens")
                .and_then(Value::as_u64)
                .filter(|value| *value > 0);
            return Capacity {
                limit,
                source: if limit.is_some() {
                    Source::Provider
                } else {
                    Source::Unknown
                },
                ollama: None,
                reasoning: provider_reasoning(&route.model_info),
                reason: limit.is_none().then(|| {
                    "The selected provider has not reported an input context limit.".into()
                }),
                identity,
            };
        };
        if route
            .litellm_params
            .api_base
            .as_deref()
            .map(|base| base.trim_end_matches('/'))
            != Some(self.ollama.as_str())
        {
            return Capacity::unknown(
                model,
                "The model's Ollama deployment does not match the configured metadata endpoint.",
            );
        }
        if native.is_empty() || native.contains('*') {
            return Capacity::unknown(
                model,
                "The selected model's native deployment could not be resolved.",
            );
        }
        let (running, shown) = tokio::join!(self.running(), self.show(native));
        let runtime = running
            .as_ref()
            .and_then(|body| body.get("models"))
            .and_then(Value::as_array)
            .and_then(|models| {
                models.iter().find(|entry| {
                    ["name", "model"]
                        .into_iter()
                        .filter_map(|key| entry.get(key).and_then(Value::as_str))
                        .any(|name| normalize(name) == normalize(native))
                })
            })
            .and_then(|entry| entry.get("context_length"))
            .and_then(Value::as_u64)
            .filter(|value| *value > 0);
        let advertised = shown.as_ref().and_then(native_limit);
        let requested = route.litellm_params.num_ctx.filter(|value| *value > 0);
        let reasoning = shown.as_ref().is_some_and(ollama_thinking);
        if let Some(runtime) = runtime {
            let limit = advertised.map_or(runtime, |advertised| runtime.min(advertised));
            return Capacity {
                limit: Some(limit),
                source: if limit == runtime {
                    Source::Runtime
                } else {
                    Source::Configured
                },
                ollama: Some(limit),
                reasoning,
                reason: (limit != runtime).then(|| {
                    "The loaded context exceeds native capacity; this request uses the supported bound.".into()
                }),
                identity,
            };
        }
        if let Some(advertised) = advertised {
            if let Some(requested) = requested {
                let limit = requested.min(advertised);
                return Capacity {
                    limit: Some(limit),
                    source: Source::Configured,
                    ollama: Some(limit),
                    reasoning,
                    reason: (limit < requested).then(|| {
                        "The requested context allocation is bounded by the model's reported native capacity.".into()
                    }),
                    identity,
                };
            }
            return Capacity {
                limit: Some(advertised),
                source: Source::Provider,
                ollama: Some(advertised),
                reasoning,
                reason: None,
                identity,
            };
        }
        let Some(limit) = requested
            .or_else(|| {
                shown
                    .as_ref()
                    .and_then(|body| body.get("parameters"))
                    .and_then(Value::as_str)
                    .and_then(configured_limit)
            })
            .or(self.configured)
        else {
            return Capacity::unknown(
                model,
                "The cold model has not reported a native capacity that can safely bound its runtime configuration.",
            );
        };
        Capacity {
            limit: Some(limit),
            source: Source::Configured,
            ollama: Some(limit),
            reasoning,
            reason: None,
            identity,
        }
    }

    async fn routes(&self, model: &str) -> Option<Vec<Route>> {
        // v1 expands wildcard deployments to example models, losing route identity.
        // Installed 1.99.1 v2 returns the actual deployment for this filter.
        tokio::time::timeout(METADATA_TIMEOUT, async {
            let mut page = 1_u64;
            let mut routes = Vec::new();
            loop {
                let body = self
                    .client
                    .get(format!(
                        "{}/v2/model/info?model={}&page={}",
                        self.host,
                        urlencoding::encode(model),
                        page
                    ))
                    .bearer_auth(&self.key)
                    .send()
                    .await
                    .ok()?
                    .error_for_status()
                    .ok()?
                    .json::<Routes>()
                    .await
                    .ok()?;
                let pages = body.total_pages.unwrap_or(1);
                routes.extend(
                    body.data
                        .into_iter()
                        .filter(|route| route.model_name == model),
                );
                if page >= pages {
                    return Some(routes);
                }
                page = page.checked_add(1)?;
            }
        })
        .await
        .ok()
        .flatten()
    }

    async fn running(&self) -> Option<Value> {
        self.client
            .get(format!("{}/api/ps", self.ollama))
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json()
            .await
            .ok()
    }

    async fn show(&self, model: &str) -> Option<Value> {
        self.client
            .post(format!("{}/api/show", self.ollama))
            .json(&serde_json::json!({"model": model}))
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json()
            .await
            .ok()
    }
}

fn select(routes: &[Route], model: &str) -> Result<Route, &'static str> {
    let exact: Vec<&Route> = routes
        .iter()
        .filter(|route| route.model_name == model)
        .collect();
    let matches = if exact.is_empty() {
        routes
            .iter()
            .filter(|route| route.model_name == "*")
            .collect::<Vec<_>>()
    } else {
        exact
    };
    let Some(first) = matches.first() else {
        return Err("The selected alias has no verified deployment route.");
    };
    if matches.iter().any(|route| {
        route.litellm_params != first.litellm_params
            || route.model_info.get("max_input_tokens") != first.model_info.get("max_input_tokens")
    }) {
        return Err("The selected alias routes to deployments with different context settings.");
    }
    let mut route = (*first).clone();
    if route.model_name == "*" {
        // The configured simple catch-all substitutes the entire submitted alias.
        if !matches!(
            route.litellm_params.model.as_str(),
            "ollama_chat/*" | "ollama/*"
        ) {
            return Err("The wildcard deployment does not expose a verifiable native model.");
        }
        route.litellm_params.model = route.litellm_params.model.replace('*', model);
    }
    Ok(route)
}

fn ollama_thinking(shown: &Value) -> bool {
    shown
        .get("capabilities")
        .and_then(Value::as_array)
        .is_some_and(|capabilities| {
            capabilities
                .iter()
                .any(|capability| capability.as_str() == Some("thinking"))
        })
}

fn provider_reasoning(info: &Value) -> bool {
    if info.get("supports_reasoning").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    info.get("supported_openai_params")
        .and_then(Value::as_array)
        .is_some_and(|parameters| {
            parameters.iter().any(|param| {
                matches!(
                    param.as_str(),
                    Some("reasoning_effort" | "thinking" | "reasoning")
                )
            })
        })
}

fn native_limit(body: &Value) -> Option<u64> {
    let metadata = body.get("model_info")?;
    let architecture = metadata.get("general.architecture")?.as_str()?;
    metadata
        .get(format!("{architecture}.context_length"))?
        .as_u64()
        .filter(|value| *value > 0)
}

fn configured_limit(parameters: &str) -> Option<u64> {
    parameters.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        (words.next()? == "num_ctx")
            .then(|| words.next()?.parse::<u64>().ok().filter(|value| *value > 0))
            .flatten()
    })
}

fn normalize(name: &str) -> String {
    if name
        .rsplit('/')
        .next()
        .is_some_and(|name| name.contains(':'))
    {
        name.into()
    } else {
        format!("{name}:latest")
    }
}

#[cfg(test)]
mod tests {
    use super::super::parameters::Parameters;
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

    #[test]
    fn wildcard_selection_does_not_override_exact_alias_or_guess_ambiguous_routes() {
        let routes: Routes = serde_json::from_value(json!({"data":[{"model_name":"*","litellm_params":{"model":"ollama_chat/*"}},{"model_name":"alias","litellm_params":{"model":"openai/remote"}}]})).unwrap();
        assert_eq!(
            select(&routes.data, "alias").unwrap().litellm_params.model,
            "openai/remote"
        );
        assert_eq!(
            select(&routes.data, "local:1b")
                .unwrap()
                .litellm_params
                .model,
            "ollama_chat/local:1b"
        );
        let mut duplicates = routes.data;
        duplicates.push(Route {
            model_name: "alias".into(),
            litellm_params: Parameters {
                model: "ollama_chat/unrelated".into(),
                api_base: None,
                num_ctx: None,
            },
            model_info: Value::Null,
        });
        assert!(select(&duplicates, "alias").is_err());
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

    #[test]
    fn thinking_is_only_the_engine_declared_capability() {
        assert!(ollama_thinking(&json!({"capabilities":["thinking"]})));
        assert!(!ollama_thinking(&json!({"capabilities":["completion"]})));
        assert!(!provider_reasoning(&json!({"supports_reasoning":false})));
        assert!(provider_reasoning(&json!({"supports_reasoning":true})));
        assert!(provider_reasoning(&json!({
            "supported_openai_params": ["temperature", "reasoning_effort"]
        })));
    }
}
