use crate::catalog::capability::ModelCapability;
use crate::catalog::capability::push_capability;
use crate::catalog::details::ModelDetails;
use crate::catalog::entry::ModelEntry;
use crate::catalog::error::CatalogError;
use crate::catalog::http::build_client;
use crate::catalog::page::ModelPage;
use crate::catalog::parse::extract_model_family;
use crate::catalog::parse::extract_param_size;
use crate::catalog::provider::ModelProvider;
use crate::catalog::query::BrowseQuery;
use crate::catalog::refine::paginate_models;
use crate::catalog::refine::parse_cursor_offset;
use crate::catalog::refine::refine_models;
use crate::catalog::text::collapse_whitespace;
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

/// Upstream OpenRouter models API.
pub const DEFAULT_OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";

/// Browses the hosted models OpenRouter routes to.
pub struct OpenRouterProvider {
    catalog_url: String,
    client: Client,
}

impl Default for OpenRouterProvider {
    fn default() -> Self {
        Self::new(DEFAULT_OPENROUTER_MODELS_URL)
    }
}

impl OpenRouterProvider {
    pub fn new(catalog_url: impl Into<String>) -> Self {
        Self::with_proxy(catalog_url, None).expect("Failed to build OpenRouter catalog client")
    }

    pub fn with_proxy(
        catalog_url: impl Into<String>,
        proxy_url: Option<&str>,
    ) -> Result<Self, CatalogError> {
        Ok(Self {
            catalog_url: catalog_url.into(),
            client: build_client(proxy_url)?,
        })
    }
}

#[async_trait]
impl ModelProvider for OpenRouterProvider {
    fn name(&self) -> &'static str {
        "openrouter"
    }

    async fn search(&self, options: BrowseQuery<'_>) -> Result<ModelPage, CatalogError> {
        let offset = parse_cursor_offset(options.cursor)?;
        let response = self.client.get(&self.catalog_url).send().await?;

        if !response.status().is_success() {
            return Err(CatalogError::Unavailable(format!(
                "OpenRouter API returned status: {}",
                response.status()
            )));
        }

        let body = response.text().await?;
        let catalog: OpenRouterResponse = serde_json::from_str(&body).map_err(|error| {
            tracing::error!(
                "OpenRouter JSON parse error: {}. Body preview: {}",
                error,
                &body[..body.len().min(500)]
            );
            CatalogError::Parse(error.to_string())
        })?;

        let matched: Vec<OpenRouterModel> = match options.query {
            Some(query) => {
                let needle = query.to_lowercase();
                catalog
                    .data
                    .into_iter()
                    .filter(|model| model.matches(&needle))
                    .collect()
            }
            None => catalog.data,
        };

        let models: Vec<ModelEntry> = matched.into_iter().map(to_model).collect();

        Ok(paginate_models(
            refine_models(models, &options),
            offset,
            options.limit,
        ))
    }
}

#[derive(Debug, Deserialize)]
struct OpenRouterResponse {
    data: Vec<OpenRouterModel>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterModel {
    id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    context_length: Option<u64>,
    #[serde(default)]
    architecture: Option<OpenRouterArchitecture>,
    #[serde(default)]
    supported_parameters: Option<Vec<String>>,
}

impl OpenRouterModel {
    fn matches(&self, needle: &str) -> bool {
        self.id.to_lowercase().contains(needle)
            || self.name.to_lowercase().contains(needle)
            || self
                .description
                .as_ref()
                .is_some_and(|value| value.to_lowercase().contains(needle))
    }
}

#[derive(Debug, Deserialize)]
struct OpenRouterArchitecture {
    #[serde(default)]
    tokenizer: Option<String>,
    #[serde(default)]
    modality: Option<String>,
    #[serde(default)]
    input_modalities: Option<Vec<String>>,
    #[serde(default)]
    output_modalities: Option<Vec<String>>,
}

fn to_model(model: OpenRouterModel) -> ModelEntry {
    let parameter_size = extract_param_size(&model.id).or_else(|| extract_param_size(&model.name));
    let family = extract_model_family(&model.id)
        .or_else(|| extract_model_family(&model.name))
        .or_else(|| {
            model
                .architecture
                .as_ref()
                .and_then(|architecture| architecture.tokenizer.clone())
        });
    let description = model
        .description
        .as_deref()
        .map(collapse_whitespace)
        .filter(|text| !text.is_empty());
    let capabilities = capabilities(&model);
    let author = model.id.split_once('/').map(|(owner, _)| owner.to_string());
    let display_name = (model.name != model.id).then(|| model.name.clone());

    ModelEntry {
        name: model.id.clone(),
        display_name,
        description,
        author,
        url: Some(format!("https://openrouter.ai/{}", model.id)),
        capabilities,
        details: Some(ModelDetails {
            format: Some("api".to_string()),
            family,
            parameter_size,
            context_length: model.context_length,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn capabilities(model: &OpenRouterModel) -> Option<Vec<ModelCapability>> {
    let mut capabilities = Vec::new();
    if let Some(architecture) = &model.architecture {
        let legacy = architecture
            .modality
            .as_deref()
            .and_then(|value| value.split_once("->"));
        for (declared, fallback, output) in [
            (
                &architecture.input_modalities,
                legacy.map(|(input, _)| input),
                false,
            ),
            (
                &architecture.output_modalities,
                legacy.map(|(_, output)| output),
                true,
            ),
        ] {
            let modalities: Vec<&str> = match declared {
                Some(values) => values.iter().map(String::as_str).collect(),
                None => fallback
                    .map(|value| value.split('+').collect())
                    .unwrap_or_default(),
            };
            for modality in modalities {
                let capability = match (modality.trim(), output) {
                    ("text", _) => Some(ModelCapability::Text),
                    ("image", false) => Some(ModelCapability::ImageInput),
                    ("image", true) => Some(ModelCapability::ImageGeneration),
                    ("audio", false) => Some(ModelCapability::AudioInput),
                    ("audio", true) => Some(ModelCapability::AudioGeneration),
                    ("video", false) => Some(ModelCapability::VideoInput),
                    ("video", true) => Some(ModelCapability::VideoGeneration),
                    _ => None,
                };
                if let Some(capability) = capability {
                    push_capability(&mut capabilities, capability);
                }
            }
        }
    }
    for parameter in model.supported_parameters.iter().flatten() {
        let capability = match parameter.as_str() {
            "tools" => Some(ModelCapability::Tools),
            "reasoning" | "include_reasoning" => Some(ModelCapability::Reasoning),
            _ => None,
        };
        if let Some(capability) = capability {
            push_capability(&mut capabilities, capability);
        }
    }
    (!capabilities.is_empty()).then_some(capabilities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::medium_filter::ModelMediumFilter;
    use crate::catalog::page::DEFAULT_PAGE_SIZE;
    use crate::catalog::size_filter::ModelSizeFilter;
    use crate::catalog::sort::ModelSort;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;

    fn browse(query: Option<&str>) -> BrowseQuery<'_> {
        BrowseQuery {
            query,
            cursor: None,
            limit: DEFAULT_PAGE_SIZE,
            sort: ModelSort::default(),
            family: None,
            size: ModelSizeFilter::default(),
            medium: ModelMediumFilter::default(),
        }
    }

    #[test]
    fn json_parsing_reads_the_data_array() {
        let json = r#"{
            "data": [
                {"id":"openai/gpt-4","name":"GPT-4","architecture":{"tokenizer":"GPT"}},
                {"id":"anthropic/claude-3","name":"Claude 3"}
            ]
        }"#;

        let response: OpenRouterResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.data.len(), 2);
        assert_eq!(response.data[0].id, "openai/gpt-4");
        assert_eq!(response.data[0].name, "GPT-4");
        assert_eq!(
            response.data[0].architecture.as_ref().unwrap().tokenizer,
            Some("GPT".to_string())
        );
        assert_eq!(response.data[1].id, "anthropic/claude-3");
        assert!(response.data[1].architecture.is_none());
    }

    #[test]
    fn capabilities_use_exact_metadata_and_direction() {
        let parsed = serde_json::from_value(serde_json::json!({
            "id": "tools-vision-reasoning/model", "name": "Image generation",
            "description": "Tools, vision, audio and reasoning are not supported",
            "architecture": {
                "modality": "image+audio->image",
                "input_modalities": ["text"], "output_modalities": ["text"]
            }, "supported_parameters": ["temperature"]
        }))
        .unwrap();
        assert_eq!(
            to_model(parsed).capabilities,
            Some(vec![ModelCapability::Text])
        );

        let parsed = serde_json::from_value(serde_json::json!({
            "id": "test/model", "name": "Test", "architecture": {
                "modality": "text+image+audio+video->text+image+audio+video"
            }, "supported_parameters": ["tools", "reasoning", "include_reasoning"]
        }))
        .unwrap();
        let capabilities = to_model(parsed).capabilities.unwrap();
        assert_eq!(capabilities.len(), 9);
        assert!(capabilities.contains(&ModelCapability::ImageInput));
        assert!(capabilities.contains(&ModelCapability::ImageGeneration));
        assert!(capabilities.contains(&ModelCapability::Tools));

        let parsed = serde_json::from_value(serde_json::json!({
            "id": "test/model", "name": "Tools image generation reasoning",
            "architecture": {"modality": "text+image->image", "input_modalities": [], "output_modalities": []}
        }))
        .unwrap();
        assert_eq!(to_model(parsed).capabilities, None);
    }

    #[test]
    fn to_model_keeps_description_and_context() {
        let json = r#"{
            "id":"anthropic/claude-sonnet-4",
            "name":"Anthropic: Claude Sonnet 4",
            "description":"A balanced model for coding and agents.",
            "context_length":200000,
            "architecture":{"tokenizer":"Claude","modality":"text+image->text","input_modalities":["text","image"]},
            "supported_parameters":["tools","include_reasoning"]
        }"#;
        let parsed: OpenRouterModel = serde_json::from_str(json).unwrap();
        let model = to_model(parsed);
        assert_eq!(model.name, "anthropic/claude-sonnet-4");
        assert_eq!(
            model.display_name.as_deref(),
            Some("Anthropic: Claude Sonnet 4")
        );
        assert_eq!(
            model.description.as_deref(),
            Some("A balanced model for coding and agents.")
        );
        assert_eq!(
            model.details.as_ref().unwrap().context_length,
            Some(200_000)
        );
        assert_eq!(
            model.capabilities.unwrap(),
            vec![
                ModelCapability::Text,
                ModelCapability::ImageInput,
                ModelCapability::Tools,
                ModelCapability::Reasoning
            ]
        );
    }

    #[tokio::test]
    async fn search_filters_the_hosted_catalog() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"id": "openai/gpt-4", "name": "GPT-4"},
                    {"id": "anthropic/claude-3", "name": "Claude 3"}
                ]
            })))
            .mount(&server)
            .await;

        let provider = OpenRouterProvider::new(server.uri());
        let page = provider.search(browse(Some("claude"))).await.unwrap();

        assert_eq!(page.models.len(), 1);
        assert_eq!(page.models[0].name, "anthropic/claude-3");
        assert_eq!(page.models[0].author.as_deref(), Some("anthropic"));
    }

    #[tokio::test]
    async fn search_reports_an_unavailable_catalog() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&server)
            .await;

        let provider = OpenRouterProvider::new(server.uri());
        let error = provider.search(browse(None)).await.unwrap_err();
        assert!(matches!(error, CatalogError::Unavailable(_)));
    }
}
