use std::time::Duration;

use abnegate_secret::SecretValue;
use reqwest::Client;
use serde_json::Value;

use super::Capacity;
use super::ChatError;
use super::Source;
use super::capacity::BOUNDED_BY_NATIVE;
use super::route::Route;
use super::routes::Routes;

/// How long a metadata request may take before capacity is reported unknown.
const METADATA_TIMEOUT: Duration = Duration::from_secs(2);

/// The Ollama `/api/show` capability an engine declares when it can think.
const THINKING: &str = "thinking";

/// The Ollama `/api/show` capability an engine declares when it can read
/// images.
const VISION: &str = "vision";

/// A LiteLLM deployment of this instance, and the Ollama behind it.
pub(super) struct Deployment {
    client: Client,
    host: String,
    key: SecretValue,
    ollama: String,
}

impl Deployment {
    /// The deployment LiteLLM serves at `host` with `key`, whose Ollama routes
    /// point at `ollama`.
    ///
    /// Fails only when no HTTP client can be built on this host, such as
    /// when its TLS backend cannot load.
    pub(super) fn new(host: &str, key: &str, ollama: &str) -> Result<Self, ChatError> {
        let client = Client::builder()
            .connect_timeout(METADATA_TIMEOUT)
            .timeout(METADATA_TIMEOUT)
            .build()?;
        Ok(Self {
            client,
            host: host.trim_end_matches('/').into(),
            key: SecretValue::new(key),
            ollama: ollama.trim_end_matches('/').into(),
        })
    }

    /// The effective context capacity of the deployment `model` routes to,
    /// falling back to the proxy's wildcard route when `model` has none, and
    /// to `configured` when a cold Ollama model reports no native capacity.
    pub(super) async fn resolve(&self, model: &str, configured: Option<u64>) -> Capacity {
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
                vision: route
                    .model_info
                    .get("supports_vision")
                    .and_then(Value::as_bool),
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
        let vision = shown.as_ref().and_then(ollama_vision);
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
                vision,
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
                    vision,
                    reason: (limit < requested).then(|| BOUNDED_BY_NATIVE.into()),
                    identity,
                };
            }
            return Capacity {
                limit: Some(advertised),
                source: Source::Provider,
                ollama: Some(advertised),
                reasoning,
                vision,
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
            .or(configured)
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
            vision,
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
                    .bearer_auth(self.key.expose())
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

fn ollama_capability(shown: &Value, name: &str) -> Option<bool> {
    shown
        .get("capabilities")
        .and_then(Value::as_array)
        .map(|capabilities| {
            capabilities
                .iter()
                .any(|capability| capability.as_str() == Some(name))
        })
}

fn ollama_thinking(shown: &Value) -> bool {
    ollama_capability(shown, THINKING).unwrap_or(false)
}

fn ollama_vision(shown: &Value) -> Option<bool> {
    ollama_capability(shown, VISION)
}

fn provider_reasoning(model: &Value) -> bool {
    if model.get("supports_reasoning").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    model
        .get("supported_openai_params")
        .and_then(Value::as_array)
        .is_some_and(|parameters| {
            parameters.iter().any(|parameter| {
                matches!(
                    parameter.as_str(),
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
    use serde_json::json;

    use super::super::parameters::Parameters;
    use super::*;

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

    #[test]
    fn vision_is_only_the_engine_declared_capability() {
        assert_eq!(
            ollama_vision(&json!({"capabilities":["vision"]})),
            Some(true)
        );
        assert_eq!(
            ollama_vision(&json!({"capabilities":["completion"]})),
            Some(false)
        );
        assert_eq!(ollama_vision(&json!({"capabilities":null})), None);
        assert_eq!(ollama_vision(&json!({})), None);
    }

    #[test]
    fn thinking_is_only_the_engine_declared_capability() {
        assert!(ollama_thinking(&json!({"capabilities":["thinking"]})));
        assert!(!ollama_thinking(&json!({"capabilities":["completion"]})));
        assert!(!ollama_thinking(&json!({})));
        assert!(!provider_reasoning(&json!({"supports_reasoning":false})));
        assert!(provider_reasoning(&json!({"supports_reasoning":true})));
        assert!(provider_reasoning(&json!({
            "supported_openai_params": ["temperature", "reasoning_effort"]
        })));
    }
}
