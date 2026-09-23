use crate::catalog::details::ModelDetails;
use crate::catalog::entry::ModelEntry;
use crate::catalog::error::CatalogError;
use crate::catalog::http::build_client;
use crate::catalog::page::ModelPage;
use crate::catalog::parse::extract_model_family;
use crate::catalog::parse::extract_param_size;
use crate::catalog::parse::extract_quantization;
use crate::catalog::parse::normalize_parameter_label;
use crate::catalog::provider::ModelProvider;
use crate::catalog::query::BrowseQuery;
use crate::catalog::refine::paginate_models;
use crate::catalog::refine::parse_cursor_offset;
use crate::catalog::refine::refine_models;
use crate::catalog::text::html_to_plain_text;
use crate::catalog::text::infer_use_cases;
use crate::catalog::text::nonempty_vec;
use crate::catalog::text::preview;
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde::Deserializer;
use serde::de;
use std::time::Duration;

/// Upstream GPT4All model catalogue.
pub const DEFAULT_GPT4ALL_MODELS_URL: &str =
    "https://raw.githubusercontent.com/nomic-ai/gpt4all/main/gpt4all-chat/metadata/models3.json";

const CATALOG_ATTEMPTS: u32 = 3;
const RETRY_BACKOFF: Duration = Duration::from_millis(50);

/// Browses the static JSON catalogue GPT4All publishes.
pub struct Gpt4AllProvider {
    catalog_url: String,
    client: Client,
}

impl Default for Gpt4AllProvider {
    fn default() -> Self {
        Self::new(DEFAULT_GPT4ALL_MODELS_URL)
    }
}

impl Gpt4AllProvider {
    pub fn new(catalog_url: impl Into<String>) -> Self {
        Self::with_proxy(catalog_url, None).expect("Failed to build GPT4All catalog client")
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
impl ModelProvider for Gpt4AllProvider {
    fn name(&self) -> &'static str {
        "gpt4all"
    }

    async fn search(&self, options: BrowseQuery<'_>) -> Result<ModelPage, CatalogError> {
        let offset = parse_cursor_offset(options.cursor)?;
        let catalog = fetch_catalog(&self.catalog_url, &self.client).await?;

        let matched: Vec<Gpt4AllModel> = match options.query {
            Some(query) => {
                let needle = query.to_lowercase();
                catalog
                    .into_iter()
                    .filter(|model| model.matches(&needle))
                    .collect()
            }
            None => catalog,
        };

        let models: Vec<ModelEntry> = matched.into_iter().map(to_model).collect();

        Ok(paginate_models(
            refine_models(models, &options),
            offset,
            options.limit,
        ))
    }
}

async fn fetch_catalog(url: &str, client: &Client) -> Result<Vec<Gpt4AllModel>, CatalogError> {
    let mut last_error: Option<CatalogError> = None;
    for attempt in 1..=CATALOG_ATTEMPTS {
        match client.get(url).send().await {
            Ok(response) if response.status().is_success() => {
                let body = response.text().await?;
                return serde_json::from_str(&body).map_err(|error| {
                    tracing::error!(
                        "GPT4All JSON parse error: {}. Body preview: {}",
                        error,
                        preview(&body)
                    );
                    CatalogError::Parse(error.to_string())
                });
            }
            Ok(response) => {
                last_error = Some(CatalogError::Unavailable(format!(
                    "GPT4All API returned status: {}",
                    response.status()
                )));
            }
            Err(error) => last_error = Some(error.into()),
        }
        if attempt < CATALOG_ATTEMPTS {
            tokio::time::sleep(RETRY_BACKOFF * attempt).await;
        }
    }
    Err(last_error.expect("at least one GPT4All catalog attempt"))
}

#[derive(Debug, Deserialize)]
struct Gpt4AllModel {
    name: String,
    filename: String,
    #[serde(deserialize_with = "string_or_number")]
    filesize: u64,
    #[serde(default)]
    parameters: Option<String>,
    #[serde(rename = "type", default)]
    model_type: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    quant: Option<String>,
    #[serde(rename = "ramrequired", default)]
    ram_required: Option<serde_json::Value>,
    #[serde(default)]
    url: Option<String>,
}

impl Gpt4AllModel {
    fn matches(&self, needle: &str) -> bool {
        self.name.to_lowercase().contains(needle)
            || self.filename.to_lowercase().contains(needle)
            || self
                .description
                .as_ref()
                .is_some_and(|value| value.to_lowercase().contains(needle))
            || self
                .model_type
                .as_ref()
                .is_some_and(|value| value.to_lowercase().contains(needle))
    }
}

fn to_model(model: Gpt4AllModel) -> ModelEntry {
    let description = model
        .description
        .as_deref()
        .map(html_to_plain_text)
        .filter(|text| !text.is_empty());
    let parameter_size = model
        .parameters
        .as_deref()
        .map(normalize_parameter_label)
        .or_else(|| extract_param_size(&model.filename));
    let quantization_level = model
        .quant
        .as_deref()
        .map(str::to_uppercase)
        .or_else(|| extract_quantization(&model.filename));
    let ram_required_gb = model.ram_required.as_ref().and_then(|value| match value {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    });
    let use_cases = nonempty_vec(infer_use_cases(&[
        description.as_deref().unwrap_or(""),
        model.model_type.as_deref().unwrap_or(""),
        &model.name,
        model.description.as_deref().unwrap_or(""),
    ]));

    ModelEntry {
        name: model.name.clone(),
        size: Some(model.filesize),
        description,
        url: model.url,
        use_cases,
        details: Some(ModelDetails {
            format: Some("gguf".to_string()),
            family: model
                .model_type
                .clone()
                .or_else(|| extract_model_family(&model.filename)),
            parameter_size,
            quantization_level,
            ram_required_gb,
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// GPT4All publishes `filesize` as a string in some entries and a number in
/// others.
fn string_or_number<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber {
        Number(u64),
        Text(String),
    }

    match StringOrNumber::deserialize(deserializer)? {
        StringOrNumber::Number(value) => Ok(value),
        StringOrNumber::Text(value) => value.parse().map_err(de::Error::custom),
    }
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
    use wiremock::matchers::path;

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
    fn json_parsing_accepts_a_string_filesize() {
        let json = r#"[
            {"name":"Test Model","filename":"test-q4_0.gguf","filesize":"4431390720","parameters":"7 billion","type":"llama"},
            {"name":"Model 2","filename":"model2.gguf","filesize":"1234567890"}
        ]"#;

        let models: Vec<Gpt4AllModel> = serde_json::from_str(json).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].name, "Test Model");
        assert_eq!(models[0].filesize, 4_431_390_720);
        assert_eq!(models[0].parameters.as_deref(), Some("7 billion"));
        assert_eq!(models[1].filesize, 1_234_567_890);
    }

    #[test]
    fn json_parsing_accepts_a_numeric_filesize() {
        let json = r#"[{"name":"Test","filename":"test.gguf","filesize":12345}]"#;
        let models: Vec<Gpt4AllModel> = serde_json::from_str(json).unwrap();
        assert_eq!(models[0].filesize, 12_345);
    }

    #[test]
    fn to_model_strips_html_and_exposes_ram() {
        let json = r#"{
            "name":"Reasoner v1",
            "filename":"qwen2.5-coder-7b-instruct-q4_0.gguf",
            "filesize":"4431390720",
            "parameters":"8 billion",
            "type":"qwen2",
            "quant":"q4_0",
            "ramrequired":"8",
            "description":"<ul><li>Use for complex reasoning tasks</li><li>#reasoning</li></ul>",
            "url":"https://example.com/model.gguf"
        }"#;
        let parsed: Gpt4AllModel = serde_json::from_str(json).unwrap();
        let model = to_model(parsed);
        let details = model.details.as_ref().expect("details");
        assert_eq!(details.parameter_size.as_deref(), Some("8B"));
        assert_eq!(details.quantization_level.as_deref(), Some("Q4_0"));
        assert_eq!(details.ram_required_gb, Some(8));
        assert!(
            model
                .description
                .as_ref()
                .unwrap()
                .contains("complex reasoning")
        );
        assert!(!model.description.as_ref().unwrap().contains("<li>"));
        assert!(
            model
                .use_cases
                .as_ref()
                .unwrap()
                .contains(&"Reasoning".to_string())
        );
    }

    #[tokio::test]
    async fn search_uses_the_injected_catalog() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models3.json"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "name": "Llama 3 Instruct",
                    "filename": "llama-3-8b-instruct.Q4_0.gguf",
                    "filesize": "4000000000",
                    "parameters": "8B",
                    "type": "LLaMA",
                    "description": "A compact Llama 3 chat model",
                    "quant": "q4_0"
                }])),
            )
            .mount(&server)
            .await;

        let provider = Gpt4AllProvider::new(format!("{}/models3.json", server.uri()));
        let page = provider.search(browse(None)).await.unwrap();

        assert_eq!(page.models.len(), 1);
        assert_eq!(page.models[0].name, "Llama 3 Instruct");
    }

    #[tokio::test]
    async fn search_filters_by_query() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"name": "Llama 3", "filename": "llama.gguf", "filesize": 1},
                {"name": "Mistral", "filename": "mistral.gguf", "filesize": 2}
            ])))
            .mount(&server)
            .await;

        let provider = Gpt4AllProvider::new(server.uri());
        let page = provider.search(browse(Some("mistral"))).await.unwrap();

        assert_eq!(page.models.len(), 1);
        assert_eq!(page.models[0].name, "Mistral");
    }

    #[tokio::test]
    async fn search_uses_the_configured_proxy() {
        let proxy = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "name": "Proxied Model",
                    "filename": "proxied-model.Q4_0.gguf",
                    "filesize": "12345"
                }])),
            )
            .mount(&proxy)
            .await;

        let provider = Gpt4AllProvider::with_proxy(
            "http://model-catalog.invalid/models3.json",
            Some(&proxy.uri()),
        )
        .unwrap();
        let page = provider.search(browse(None)).await.unwrap();

        assert_eq!(page.models.len(), 1);
        assert_eq!(page.models[0].name, "Proxied Model");
    }

    #[tokio::test]
    async fn search_retries_then_reports_an_unavailable_catalog() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .expect(u64::from(CATALOG_ATTEMPTS))
            .mount(&server)
            .await;

        let provider = Gpt4AllProvider::new(server.uri());
        let error = provider.search(browse(None)).await.unwrap_err();

        assert!(matches!(error, CatalogError::Unavailable(_)));
    }

    #[tokio::test]
    async fn a_malformed_body_with_a_multibyte_character_at_the_preview_limit_is_a_parse_error() {
        let _listening = tracing::subscriber::set_default(crate::catalog::listening::Listening);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(format!("{}é", "a".repeat(499))),
            )
            .mount(&server)
            .await;

        let provider = Gpt4AllProvider::new(server.uri());
        let error = provider.search(browse(None)).await.unwrap_err();

        assert!(matches!(error, CatalogError::Parse(_)), "{error:?}");
    }
}
