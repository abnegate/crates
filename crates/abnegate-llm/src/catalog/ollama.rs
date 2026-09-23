use crate::catalog::capability::declared_capabilities;
use crate::catalog::details::ModelDetails;
use crate::catalog::entry::ModelEntry;
use crate::catalog::error::CatalogError;
use crate::catalog::http::build_client;
use crate::catalog::medium_filter::ModelMediumFilter;
use crate::catalog::page::ModelPage;
use crate::catalog::parse::collect_param_size_labels;
use crate::catalog::parse::extract_model_family;
use crate::catalog::parse::extract_param_size;
use crate::catalog::parse::format_param_sizes;
use crate::catalog::parse::is_param_size_chip;
use crate::catalog::parse::parse_compact_count;
use crate::catalog::provider::ModelProvider;
use crate::catalog::query::BrowseQuery;
use crate::catalog::refine::paginate_models;
use crate::catalog::refine::parse_cursor_offset;
use crate::catalog::refine::refine_models;
use crate::catalog::size::ModelSize;
use crate::catalog::sort::ModelSort;
use crate::catalog::text::collapse_whitespace;
use crate::catalog::text::infer_use_cases;
use crate::catalog::text::nonempty_vec;
use async_trait::async_trait;
use futures::stream;
use futures::stream::StreamExt;
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::Client;
use scraper::Html;
use scraper::Selector;
use std::collections::HashSet;

/// Ollama's own catalogue search page.
pub const DEFAULT_OLLAMA_SEARCH_URL: &str = "https://ollama.com/search";
/// Ollama publishes exact blob sizes in its registry manifests; the library
/// listing page carries no size at all, so each browsed model needs one lookup.
/// Official models live under `library/` in it and community models under
/// their owner.
pub const DEFAULT_OLLAMA_REGISTRY_URL: &str = "https://registry.ollama.ai/v2";

const OFFICIAL_NAMESPACE: &str = "library";

const SIZE_LOOKUP_CONCURRENCY: usize = 8;

static PULLS_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)([\d,.]+[KMBkmb]?)\s*Pulls").expect("pulls regex"));

/// Browses the models Ollama lists in its library.
pub struct OllamaProvider {
    client: Client,
    search_url: String,
    registry_url: String,
}

impl OllamaProvider {
    pub fn new() -> Self {
        Self::with_proxy(None).expect("Failed to build Ollama model catalog client")
    }

    pub fn with_proxy(proxy_url: Option<&str>) -> Result<Self, CatalogError> {
        Self::with_catalog(
            DEFAULT_OLLAMA_SEARCH_URL,
            DEFAULT_OLLAMA_REGISTRY_URL,
            proxy_url,
        )
    }

    pub fn with_catalog(
        search_url: impl Into<String>,
        registry_url: impl Into<String>,
        proxy_url: Option<&str>,
    ) -> Result<Self, CatalogError> {
        Ok(Self {
            client: build_client(proxy_url)?,
            search_url: search_url.into(),
            registry_url: registry_url.into(),
        })
    }
}

impl Default for OllamaProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for OllamaProvider {
    fn name(&self) -> &'static str {
        "ollama"
    }

    async fn search(&self, options: BrowseQuery<'_>) -> Result<ModelPage, CatalogError> {
        let offset = parse_cursor_offset(options.cursor, options.limit)?;
        let url = search_url(
            &self.search_url,
            options.query,
            options.family,
            options.medium,
        );
        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(CatalogError::Unavailable(format!(
                "Ollama library returned status: {}",
                response.status()
            )));
        }

        let html = response.text().await?;
        let mut models = parse_library_html(&html);
        if matches!(options.sort, ModelSort::SizeAsc | ModelSort::SizeDesc) {
            models = attach_download_sizes(models, &self.client, &self.registry_url).await;
            models = refine_models(models, &options);
            return Ok(paginate_models(models, offset, options.limit));
        }

        models = refine_models(models, &options);
        let mut page = paginate_models(models, offset, options.limit);
        page.models = attach_download_sizes(page.models, &self.client, &self.registry_url).await;
        Ok(page)
    }
}

/// A model whose manifest cannot be fetched keeps `size: None` rather than
/// failing the whole listing.
async fn attach_download_sizes(
    models: Vec<ModelEntry>,
    client: &Client,
    registry_url: &str,
) -> Vec<ModelEntry> {
    stream::iter(models)
        .map(|mut model| {
            let client = client.clone();
            async move {
                model.size = fetch_manifest_size(&model.name, &client, registry_url).await;
                if let Some(sizes) = model.sizes.as_mut() {
                    for variant in sizes.iter_mut() {
                        variant.size =
                            fetch_manifest_size(&variant.name, &client, registry_url).await;
                    }
                }
                model
            }
        })
        .buffered(SIZE_LOOKUP_CONCURRENCY)
        .collect()
        .await
}

fn search_url(
    base_url: &str,
    query: Option<&str>,
    family: Option<&str>,
    medium: ModelMediumFilter,
) -> String {
    let mut terms = Vec::new();
    if let Some(query) = query.map(str::trim).filter(|value| !value.is_empty()) {
        terms.push(query);
    }
    if let Some(family) = family.map(str::trim).filter(|value| !value.is_empty())
        && !terms.iter().any(|term| term.eq_ignore_ascii_case(family))
    {
        terms.push(family);
    }

    let mut url = if terms.is_empty() {
        base_url.to_string()
    } else {
        format!("{base_url}?q={}", urlencoding::encode(&terms.join(" ")))
    };

    if let Some(category) = medium_category(medium) {
        url.push_str(if terms.is_empty() { "?c=" } else { "&c=" });
        url.push_str(category);
    }

    url
}

fn medium_category(medium: ModelMediumFilter) -> Option<&'static str> {
    match medium {
        ModelMediumFilter::Image => Some("vision"),
        ModelMediumFilter::Tools => Some("tools"),
        ModelMediumFilter::Embeddings => Some("embedding"),
        ModelMediumFilter::Reasoning => Some("thinking"),
        ModelMediumFilter::All
        | ModelMediumFilter::Text
        | ModelMediumFilter::ImageGeneration
        | ModelMediumFilter::Video
        | ModelMediumFilter::Audio => None,
    }
}

/// Split `llama3.2:1b` into repository and tag. Untagged names use `latest`.
fn manifest_reference(name: &str) -> (&str, &str) {
    match name.split_once(':') {
        Some((repository, tag)) if !repository.is_empty() && !tag.is_empty() => (repository, tag),
        _ => (name, "latest"),
    }
}

/// The registry manifest for `name`: `library/{name}` for an official model,
/// `{owner}/{model}` for a community one.
fn manifest_url(registry_url: &str, name: &str) -> String {
    let (repository, tag) = manifest_reference(name);
    let registry_url = registry_url.trim_end_matches('/');
    if repository.contains('/') {
        format!("{registry_url}/{repository}/manifests/{tag}")
    } else {
        format!("{registry_url}/{OFFICIAL_NAMESPACE}/{repository}/manifests/{tag}")
    }
}

async fn fetch_manifest_size(name: &str, client: &Client, registry_url: &str) -> Option<u64> {
    let url = manifest_url(registry_url, name);

    let response = client
        .get(&url)
        .header(
            "Accept",
            "application/vnd.docker.distribution.manifest.v2+json",
        )
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        return None;
    }

    let manifest: serde_json::Value = response.json().await.ok()?;
    let total: u64 = manifest
        .get("layers")?
        .as_array()?
        .iter()
        .filter_map(|layer| layer.get("size")?.as_u64())
        .sum();

    (total > 0).then_some(total)
}

fn is_site_segment(segment: &str) -> bool {
    matches!(
        segment.to_ascii_lowercase().as_str(),
        "search"
            | "docs"
            | "signin"
            | "sign-in"
            | "login"
            | "signup"
            | "sign-up"
            | "download"
            | "pricing"
            | "blog"
            | "public"
            | "tags"
            | "rss"
            | "privacy"
            | "terms"
            | "about"
    )
}

/// Official cards are `/library/{name}`; community cards are `/{owner}/{model}`.
fn catalog_entry(href: &str) -> Option<(String, String)> {
    let path = href.split(['?', '#']).next().unwrap_or(href);
    let path = path.strip_prefix('/').unwrap_or(path);
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    match segments.as_slice() {
        ["library", name] => Some((
            (*name).to_string(),
            format!("https://ollama.com/library/{name}"),
        )),
        [owner, name] if !is_site_segment(owner) && *owner != "library" => Some((
            format!("{owner}/{name}"),
            format!("https://ollama.com/{owner}/{name}"),
        )),
        _ => None,
    }
}

fn parse_library_html(html: &str) -> Vec<ModelEntry> {
    let document = Html::parse_document(html);
    let mut models = Vec::new();

    let card = Selector::parse("a[href^='/']").expect("static selector");
    let paragraph = Selector::parse("p").expect("static selector");
    let span = Selector::parse("span").expect("static selector");

    for element in document.select(&card) {
        let Some((name, url)) = element.value().attr("href").and_then(catalog_entry) else {
            continue;
        };

        let text = collapse_whitespace(&element.text().collect::<Vec<_>>().join(" "));
        let description = element
            .select(&paragraph)
            .map(|paragraph| collapse_whitespace(&paragraph.text().collect::<String>()))
            .find(|line| !line.is_empty() && !is_stat_line(line));
        let capability_tags = element
            .select(&span)
            .map(|span| collapse_whitespace(&span.text().collect::<String>()))
            .filter(|tag| is_capability_tag(tag))
            .collect::<Vec<_>>();
        let chips = element
            .select(&span)
            .map(|span| collapse_whitespace(&span.text().collect::<String>()))
            .filter(|chip| is_param_size_chip(chip))
            .collect::<Vec<_>>();

        let labels = collect_param_size_labels(&text, chips);
        let parameter_size = format_param_sizes(labels.clone())
            .or_else(|| description.as_deref().and_then(extract_param_size));
        let sizes = size_variants(&name, &labels);
        let family = extract_model_family(&name);
        let use_cases = nonempty_vec(infer_use_cases(&[
            description.as_deref().unwrap_or(""),
            &capability_tags.join(" "),
            &name,
        ]));
        let capabilities = declared_capabilities(capability_tags.iter().map(String::as_str));
        let tags = nonempty_vec(capability_tags);
        let downloads = extract_pulls(&text);

        models.push(ModelEntry {
            name,
            description,
            url: Some(url),
            downloads,
            tags,
            use_cases,
            capabilities,
            sizes,
            details: Some(ModelDetails {
                format: Some("gguf".to_string()),
                family,
                parameter_size,
                ..Default::default()
            }),
            ..Default::default()
        });
    }

    // Ollama's own ranking is load-bearing: an alphabetical sort buries current
    // official models behind older names that happen to sort earlier.
    let mut seen = HashSet::new();
    models.retain(|model| seen.insert(model.name.clone()));
    models
}

fn size_tag(label: &str) -> String {
    label.trim().to_lowercase()
}

/// Build pullable size options when a library card lists more than one.
fn size_variants(name: &str, labels: &[String]) -> Option<Vec<ModelSize>> {
    if labels.len() < 2 || name.contains(':') {
        return None;
    }
    Some(
        labels
            .iter()
            .map(|label| ModelSize {
                name: format!("{name}:{}", size_tag(label)),
                label: label.clone(),
                size: None,
            })
            .collect(),
    )
}

fn is_capability_tag(text: &str) -> bool {
    matches!(
        text.trim().to_lowercase().as_str(),
        "tools" | "thinking" | "vision" | "embedding" | "embeddings" | "audio" | "code" | "cloud"
    )
}

fn is_stat_line(text: &str) -> bool {
    let lowered = text.to_lowercase();
    lowered.contains("pulls") || lowered.contains("updated") || lowered.contains("tag")
}

fn extract_pulls(text: &str) -> Option<u64> {
    PULLS_RE
        .captures(text)
        .and_then(|capture| parse_compact_count(&capture[1]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::capability::ModelCapability;
    use crate::catalog::medium_filter::ModelMediumFilter;
    use crate::catalog::size_filter::ModelSizeFilter;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    #[test]
    fn search_url_includes_family_when_query_is_empty() {
        let base = DEFAULT_OLLAMA_SEARCH_URL;
        assert_eq!(
            search_url(base, None, None, ModelMediumFilter::All),
            "https://ollama.com/search"
        );
        assert_eq!(
            search_url(base, Some(""), Some("llama"), ModelMediumFilter::All),
            "https://ollama.com/search?q=llama"
        );
        assert_eq!(
            search_url(base, Some("vision"), Some("llama"), ModelMediumFilter::All),
            "https://ollama.com/search?q=vision%20llama"
        );
        assert_eq!(
            search_url(base, Some("llama"), Some("llama"), ModelMediumFilter::All),
            "https://ollama.com/search?q=llama"
        );
        assert_eq!(
            search_url(base, None, None, ModelMediumFilter::Image),
            "https://ollama.com/search?c=vision"
        );
        assert_eq!(
            search_url(base, Some("llama"), None, ModelMediumFilter::Tools),
            "https://ollama.com/search?q=llama&c=tools"
        );
        assert_eq!(
            search_url(base, None, Some("qwen"), ModelMediumFilter::Embeddings),
            "https://ollama.com/search?q=qwen&c=embedding"
        );
        assert_eq!(
            search_url(base, None, None, ModelMediumFilter::Reasoning),
            "https://ollama.com/search?c=thinking"
        );
        assert_eq!(
            search_url(base, None, None, ModelMediumFilter::Text),
            "https://ollama.com/search"
        );
    }

    #[test]
    fn manifest_reference_defaults_to_latest() {
        assert_eq!(manifest_reference("llama3.2"), ("llama3.2", "latest"));
        assert_eq!(manifest_reference("llama3.2:1b"), ("llama3.2", "1b"));
        assert_eq!(manifest_reference("llama3.1:70b"), ("llama3.1", "70b"));
        assert_eq!(manifest_reference("model:"), ("model:", "latest"));
    }

    #[test]
    fn a_community_model_is_looked_up_under_its_owner_not_the_library() {
        assert_eq!(
            manifest_url(DEFAULT_OLLAMA_REGISTRY_URL, "llama3.2:1b"),
            "https://registry.ollama.ai/v2/library/llama3.2/manifests/1b"
        );
        assert_eq!(
            manifest_url(DEFAULT_OLLAMA_REGISTRY_URL, "someone/custom"),
            "https://registry.ollama.ai/v2/someone/custom/manifests/latest"
        );
        assert_eq!(
            manifest_url("http://registry.test/v2/", "someone/custom:q4"),
            "http://registry.test/v2/someone/custom/manifests/q4"
        );
    }

    #[test]
    fn size_variants_need_more_than_one_label() {
        let variants = size_variants("llama3.2", &["1B".into(), "3B".into()]).unwrap();
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].name, "llama3.2:1b");
        assert_eq!(variants[1].name, "llama3.2:3b");
        assert!(size_variants("llama3.2", &["3B".into()]).is_none());
        assert!(size_variants("llama3.1:70b", &["8B".into(), "70B".into()]).is_none());
    }

    #[test]
    fn parse_library_html_reads_cards() {
        let html = r#"
            <html>
                <body>
                    <a href="/library/llama3.2">Llama 3.2 3B</a>
                    <a href="/library/mistral">Mistral 7B</a>
                    <a href="/library/qwen2.5">Qwen 2.5 7B</a>
                </body>
            </html>
        "#;

        let models = parse_library_html(html);
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].name, "llama3.2");
        assert_eq!(models[1].name, "mistral");
        assert_eq!(models[2].name, "qwen2.5");

        for model in &models {
            let details = model.details.as_ref().expect("details");
            assert_eq!(details.format, Some("gguf".to_string()));
        }
    }

    #[test]
    fn a_page_with_no_cards_lists_no_models_rather_than_inventing_some() {
        assert!(parse_library_html("<html><body></body></html>").is_empty());
        assert!(parse_library_html(r#"<a href="/search">Search</a>"#).is_empty());
    }

    #[test]
    fn parse_library_html_preserves_catalog_order() {
        let html = r#"
            <a href="/library/qwen3.8"><p>Qwen3.8</p></a>
            <a href="/library/qwen3.5"><p>Qwen 3.5</p></a>
            <a href="/library/qwen2.5"><p>Qwen 2.5</p></a>
            <a href="/library/codeqwen"><p>CodeQwen</p></a>
            <a href="/library/qwen3.8"><p>duplicate</p></a>
        "#;
        let models = parse_library_html(html);
        assert_eq!(
            models
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec!["qwen3.8", "qwen3.5", "qwen2.5", "codeqwen"]
        );
    }

    #[test]
    fn parse_library_html_keeps_namespaced_models() {
        let html = r#"
            <a href="/search">Search</a>
            <a href="/docs">Docs</a>
            <a href="/signin">Sign in</a>
            <a href="/download">Download</a>
            <a href="/pricing">Pricing</a>
            <a href="/library">Library</a>
            <a href="/library/qwen3.8"><p>Qwen3.8</p></a>
            <a href="/someone/custom"><p>Custom</p></a>
            <a href="/blog">Blog</a>
        "#;
        let models = parse_library_html(html);
        assert_eq!(
            models
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec!["qwen3.8", "someone/custom"]
        );
        assert_eq!(
            models[0].url.as_deref(),
            Some("https://ollama.com/library/qwen3.8")
        );
        assert_eq!(
            models[1].url.as_deref(),
            Some("https://ollama.com/someone/custom")
        );
    }

    #[test]
    fn parse_library_html_deduplicates() {
        let html = r#"
            <html>
                <body>
                    <a href="/library/llama3.2">Llama 3.2</a>
                    <a href="/library/llama3.2">Llama 3.2 Duplicate</a>
                    <a href="/library/mistral">Mistral</a>
                </body>
            </html>
        "#;

        let models = parse_library_html(html);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].name, "llama3.2");
        assert_eq!(models[1].name, "mistral");
    }

    #[test]
    fn parse_library_html_extracts_description_and_use_cases() {
        let html = r#"
            <ul role="list">
              <li>
                <a href="/library/llama3.2">
                  <h2><span>llama3.2</span></h2>
                  <p class="max-w-lg">Meta's compact Llama 3.2 for multilingual dialogue and agents.</p>
                  <span>tools</span>
                  <span>3B</span>
                  <span>1B</span>
                  <p><span>28.7K</span><span>Pulls</span></p>
                </a>
              </li>
            </ul>
        "#;

        let models = parse_library_html(html);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "llama3.2");
        assert_eq!(
            models[0].description.as_deref(),
            Some("Meta's compact Llama 3.2 for multilingual dialogue and agents.")
        );
        assert_eq!(
            models[0]
                .details
                .as_ref()
                .unwrap()
                .parameter_size
                .as_deref(),
            Some("1B · 3B")
        );
        let sizes = models[0].sizes.as_ref().expect("multiple sizes");
        assert_eq!(sizes.len(), 2);
        assert_eq!(sizes[0].name, "llama3.2:1b");
        assert_eq!(sizes[0].label, "1B");
        assert_eq!(sizes[1].name, "llama3.2:3b");
        assert_eq!(sizes[1].label, "3B");
        assert_eq!(models[0].downloads, Some(28_700));
        assert!(
            models[0]
                .use_cases
                .as_ref()
                .unwrap()
                .iter()
                .any(|case| case == "Tool use" || case == "Agents" || case == "Chat")
        );
    }

    #[test]
    fn capabilities_require_exact_chips() {
        let models = parse_library_html(
            r#"<a href="/library/plain"><p>Tools vision image generation audio reasoning</p></a>
            <a href="/library/declared"><p>A model</p><span>tools</span><span>vision</span><span>thinking</span></a>"#,
        );
        assert_eq!(
            models
                .iter()
                .find(|model| model.name == "plain")
                .unwrap()
                .capabilities,
            None
        );
        assert_eq!(
            models
                .iter()
                .find(|model| model.name == "declared")
                .unwrap()
                .capabilities,
            Some(vec![
                ModelCapability::Tools,
                ModelCapability::ImageInput,
                ModelCapability::Reasoning
            ])
        );
    }

    #[tokio::test]
    async fn search_reads_the_library_page_and_registry_sizes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<a href="/library/llama3.2"><p>Meta's compact model</p></a>
                    <a href="/someone/custom"><p>A community model</p></a>"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v2/library/llama3.2/manifests/latest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "layers": [{"size": 1_000_u64}, {"size": 2_000_u64}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v2/someone/custom/manifests/latest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "layers": [{"size": 2_000_u64}, {"size": 3_000_u64}]
            })))
            .mount(&server)
            .await;

        let provider = OllamaProvider::with_catalog(
            format!("{}/search", server.uri()),
            format!("{}/v2", server.uri()),
            None,
        )
        .unwrap();
        let page = provider
            .search(BrowseQuery {
                query: None,
                cursor: None,
                limit: 20,
                sort: ModelSort::Relevance,
                family: None,
                size: ModelSizeFilter::All,
                medium: ModelMediumFilter::All,
            })
            .await
            .unwrap();

        assert_eq!(page.models.len(), 2);
        assert_eq!(page.models[0].name, "llama3.2");
        assert_eq!(page.models[0].size, Some(3_000));
        assert_eq!(page.models[1].name, "someone/custom");
        assert_eq!(page.models[1].size, Some(5_000));
    }

    #[tokio::test]
    async fn a_search_that_matches_nothing_returns_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<html><body><p>No models found</p><a href="/signin">Sign in</a></body></html>"#,
            ))
            .mount(&server)
            .await;
        let provider = OllamaProvider::with_catalog(
            format!("{}/search", server.uri()),
            format!("{}/v2", server.uri()),
            None,
        )
        .unwrap();

        let page = provider
            .search(BrowseQuery {
                query: Some("zzz-no-such-model"),
                cursor: None,
                limit: 20,
                sort: ModelSort::Relevance,
                family: None,
                size: ModelSizeFilter::All,
                medium: ModelMediumFilter::All,
            })
            .await
            .unwrap();

        assert!(page.models.is_empty(), "{:?}", page.models);
        assert!(page.next_cursor.is_none());
    }

    #[tokio::test]
    async fn search_reports_an_unavailable_library() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let provider =
            OllamaProvider::with_catalog(server.uri(), DEFAULT_OLLAMA_REGISTRY_URL, None).unwrap();
        let error = provider
            .search(BrowseQuery {
                query: None,
                cursor: None,
                limit: 20,
                sort: ModelSort::Relevance,
                family: None,
                size: ModelSizeFilter::All,
                medium: ModelMediumFilter::All,
            })
            .await
            .unwrap_err();

        assert!(matches!(error, CatalogError::Unavailable(_)));
    }
}
