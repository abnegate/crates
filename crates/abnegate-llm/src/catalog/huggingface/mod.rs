mod card_data;
mod gguf;
mod model;
mod sibling;
mod variant;

use crate::catalog::capability::ModelCapability;
use crate::catalog::capability::declared_capabilities;
use crate::catalog::details::ModelDetails;
use crate::catalog::entry::ModelEntry;
use crate::catalog::error::CatalogError;
use crate::catalog::http::build_client;
use crate::catalog::huggingface::model::HuggingFaceModel;
use crate::catalog::huggingface::sibling::HuggingFaceSibling;
use crate::catalog::huggingface::variant::GgufVariant;
use crate::catalog::medium_filter::ModelMediumFilter;
use crate::catalog::page::MAXIMUM_PAGE_SIZE;
use crate::catalog::page::ModelPage;
use crate::catalog::parse::download_parameter_billions;
use crate::catalog::parse::extract_all_parameter_sizes;
use crate::catalog::parse::extract_model_family;
use crate::catalog::parse::extract_parameter_size;
use crate::catalog::parse::extract_quantization;
use crate::catalog::parse::is_parameter_size_chip;
use crate::catalog::parse::quantization_bit_width;
use crate::catalog::parse::quantization_preference;
use crate::catalog::provider::ModelProvider;
use crate::catalog::query::BrowseQuery;
use crate::catalog::refine::compare;
use crate::catalog::refine::count_matching_models;
use crate::catalog::refine::paginate_models;
use crate::catalog::refine::parse_cursor_offset;
use crate::catalog::refine::refine_models;
use crate::catalog::size::ModelSize;
use crate::catalog::size_filter::ModelSizeFilter;
use crate::catalog::sort::ModelSort;
use crate::catalog::text::format_context_tokens;
use crate::catalog::text::humanize_label;
use crate::catalog::text::nonempty_vec;
use crate::catalog::text::preview;
use crate::catalog::text::use_cases_from_pipeline;
use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use reqwest::Url;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::LazyLock;

/// Upstream HuggingFace models API.
pub const DEFAULT_HUGGINGFACE_MODELS_URL: &str = "https://huggingface.co/api/models";

/// Pages fetched when HuggingFace cannot apply the requested sort natively.
const WINDOW_PAGES: usize = 5;
/// Extra scan budget for size filters, which can skip most downloads-ranked rows.
const FILTER_MAXIMUM_PAGES: usize = 15;

static GGUF_SHARD_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)-\d{5}-of-\d{5}$").expect("gguf shard pattern"));

const OFFSET_PREFIX: &str = "offset:";
const EXPAND_PARAMETER: &str = "expand[]";
const EXPANDED_FIELDS: &[&str] = &[
    "cardData",
    "gguf",
    "downloads",
    "likes",
    "tags",
    "pipeline_tag",
    "createdAt",
    "author",
    "lastModified",
    "siblings",
];

/// Browses the GGUF and adapter repositories HuggingFace publishes.
#[derive(Debug, Clone)]
pub struct HuggingFaceProvider {
    catalog_url: String,
    client: Client,
}

impl HuggingFaceProvider {
    /// A client for `catalog_url`, failing only if no HTTP client can be
    /// built on this platform.
    pub fn new(catalog_url: impl Into<String>) -> Result<Self, CatalogError> {
        Self::with_proxy(catalog_url, None)
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

    /// Adapters published against the given base models, newest downloads first.
    pub async fn search_adapters(
        &self,
        options: BrowseQuery<'_>,
        bases: &[String],
    ) -> Result<Vec<ModelEntry>, CatalogError> {
        if bases.is_empty() {
            return Ok(Vec::new());
        }
        let mut models = Vec::new();
        for base in bases {
            let page = fetch_adapter_page(
                &self.catalog_url,
                &self.client,
                options.query,
                base,
                options.page_size(),
            )
            .await?;
            models.extend(page);
        }
        models.sort_by(|left, right| {
            right
                .downloads
                .unwrap_or(0)
                .cmp(&left.downloads.unwrap_or(0))
        });
        models.dedup_by(|left, right| left.name == right.name);
        Ok(refine_models(models, &options))
    }
}

#[async_trait]
impl ModelProvider for HuggingFaceProvider {
    fn name(&self) -> &'static str {
        "huggingface"
    }

    async fn search(&self, options: BrowseQuery<'_>) -> Result<ModelPage, CatalogError> {
        let page_size = options.page_size();
        if options.medium == ModelMediumFilter::ImageGeneration {
            return Ok(ModelPage::default());
        }
        if !uses_local_window(&options) {
            let (models, next_cursor) = fetch_page(
                &self.catalog_url,
                &self.client,
                &options,
                options.cursor,
                page_size,
            )
            .await?;
            return Ok(ModelPage {
                models: refine_models(models, &options),
                next_cursor,
            });
        }

        // Size and medium filters, and name/size/parameter sorts, cannot be
        // applied to a single downloads-ranked page. Gather a window first,
        // refine it, then paginate with an offset cursor.
        let offset = parse_cursor_offset(options.cursor, page_size).unwrap_or(0);
        let needs_sorted_window = uses_local_sort(options.sort);
        let maximum_pages = window_pages(needs_sorted_window);
        let mut accumulated = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0;

        loop {
            let (page, next) = fetch_page(
                &self.catalog_url,
                &self.client,
                &options,
                cursor.as_deref(),
                MAXIMUM_PAGE_SIZE,
            )
            .await?;
            pages += 1;
            accumulated.extend(page);
            cursor = next;

            if cursor.is_none() || pages >= maximum_pages {
                break;
            }

            // Size-only refinement can stop once this window can fill the page.
            // Name, size and parameter sorts need the full window before ordering.
            if !needs_sorted_window
                && count_matching_models(&accumulated, &options) >= offset.saturating_add(page_size)
            {
                break;
            }
        }

        let mut page = paginate_models(refine_models(accumulated, &options), offset, page_size);
        page.next_cursor = window_next_cursor(
            page.next_cursor,
            cursor.is_some(),
            needs_sorted_window,
            pages >= maximum_pages,
            offset,
            page.models.len(),
        );
        Ok(page)
    }
}

fn window_pages(needs_sorted_window: bool) -> usize {
    if needs_sorted_window {
        WINDOW_PAGES
    } else {
        FILTER_MAXIMUM_PAGES
    }
}

/// Keep pagination inside the fetched window for local sorts. Size filters may
/// continue only while we stopped early with more HuggingFace pages available.
fn window_next_cursor(
    page_next: Option<String>,
    has_more: bool,
    needs_sorted_window: bool,
    hit_page_cap: bool,
    offset: usize,
    page_len: usize,
) -> Option<String> {
    if page_len == 0 {
        return None;
    }
    if page_next.is_some() {
        return page_next;
    }
    if has_more && !needs_sorted_window && !hit_page_cap {
        return Some(format!("offset:{}", offset + page_len));
    }
    None
}

fn uses_local_sort(sort: ModelSort) -> bool {
    matches!(
        sort,
        ModelSort::NameAscending
            | ModelSort::NameDescending
            | ModelSort::SizeAscending
            | ModelSort::SizeDescending
            | ModelSort::ParametersAscending
            | ModelSort::ParametersDescending
    )
}

fn uses_local_window(options: &BrowseQuery<'_>) -> bool {
    options.size != ModelSizeFilter::All
        || options.medium != ModelMediumFilter::All
        || uses_local_sort(options.sort)
}

fn medium_tag(medium: ModelMediumFilter) -> Option<&'static str> {
    match medium {
        ModelMediumFilter::Image => Some("image-text-to-text"),
        ModelMediumFilter::Video => Some("video-text-to-text"),
        ModelMediumFilter::Audio => Some("automatic-speech-recognition"),
        ModelMediumFilter::Embeddings => Some("feature-extraction"),
        ModelMediumFilter::All
        | ModelMediumFilter::ImageGeneration
        | ModelMediumFilter::Text
        | ModelMediumFilter::Tools
        | ModelMediumFilter::Reasoning => None,
    }
}

fn sort_parameters(sort: ModelSort) -> (&'static str, i8) {
    match sort {
        ModelSort::UpdatedAscending => ("lastModified", 1),
        ModelSort::UpdatedDescending => ("lastModified", -1),
        ModelSort::DownloadsAscending => ("downloads", 1),
        ModelSort::Relevance
        | ModelSort::DownloadsDescending
        | ModelSort::NameAscending
        | ModelSort::NameDescending
        | ModelSort::SizeAscending
        | ModelSort::SizeDescending
        | ModelSort::ParametersAscending
        | ModelSort::ParametersDescending => ("downloads", -1),
    }
}

fn catalogue_url(catalog_url: &str) -> Result<Url, CatalogError> {
    Url::parse(catalog_url)
        .map_err(|error| CatalogError::InvalidUrl(format!("{catalog_url}: {error}")))
}

/// The models API query for one page.
///
/// Every value is percent-encoded by the URL builder, so a cursor or a search
/// holding `&` or `=` stays one parameter. `offset:N`, and a cursor that is
/// only digits, page by offset; any other cursor is HuggingFace's own.
fn search_url(
    catalog_url: &str,
    options: &BrowseQuery<'_>,
    cursor: Option<&str>,
    limit: usize,
) -> Result<Url, CatalogError> {
    let mut url = catalogue_url(catalog_url)?;
    let (sort_field, direction) = sort_parameters(options.sort);
    let offset = cursor.map(cursor_offset).transpose()?.flatten();
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("filter", "gguf")
            .append_pair("sort", sort_field)
            .append_pair("direction", &direction.to_string())
            .append_pair("limit", &limit.to_string());
        for field in EXPANDED_FIELDS {
            query.append_pair(EXPAND_PARAMETER, field);
        }

        match (offset, cursor) {
            (Some(offset), _) => {
                query.append_pair("offset", &offset.to_string());
            }
            (None, Some(cursor)) => {
                query.append_pair("cursor", cursor);
            }
            (None, None) => {}
        }

        let search = options
            .query
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                options
                    .family
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            });
        if let Some(search) = search {
            query.append_pair("search", search);
        }

        if let Some(tag) = medium_tag(options.medium) {
            query.append_pair("filter", tag);
        }
    }
    Ok(url)
}

/// The offset a cursor names, or `None` for an opaque HuggingFace cursor.
fn cursor_offset(cursor: &str) -> Result<Option<usize>, CatalogError> {
    if let Some(offset) = cursor.strip_prefix(OFFSET_PREFIX) {
        return offset
            .parse()
            .map(Some)
            .map_err(|_| CatalogError::Parse(format!("Invalid cursor offset: {cursor}")));
    }
    if !cursor.is_empty() && cursor.bytes().all(|byte| byte.is_ascii_digit()) {
        return cursor
            .parse()
            .map(Some)
            .map_err(|_| CatalogError::Parse(format!("Invalid cursor offset: {cursor}")));
    }
    Ok(None)
}

async fn fetch_page(
    catalog_url: &str,
    client: &Client,
    options: &BrowseQuery<'_>,
    cursor: Option<&str>,
    limit: usize,
) -> Result<(Vec<ModelEntry>, Option<String>), CatalogError> {
    let url = search_url(catalog_url, options, cursor, limit)?;
    let response = client.get(url).send().await?;

    if !response.status().is_success() {
        return Err(CatalogError::Unavailable(format!(
            "HuggingFace API returned status: {}",
            response.status()
        )));
    }

    let next_cursor = response
        .headers()
        .get("link")
        .and_then(|value| value.to_str().ok())
        .and_then(extract_cursor_from_link_header);
    let body = response.text().await?;
    let models: Vec<HuggingFaceModel> = serde_json::from_str(&body).map_err(|error| {
        tracing::error!(
            "HuggingFace JSON parse error: {}. Body preview: {}",
            error,
            preview(&body)
        );
        CatalogError::Parse(error.to_string())
    })?;

    Ok((models.into_iter().map(to_model).collect(), next_cursor))
}

/// Read the `rel="next"` link's pagination token.
///
/// The models API answers with a `cursor`; the repository listings answer with
/// a numeric `offset`, which becomes this crate's own `offset:` cursor.
fn extract_cursor_from_link_header(link: &str) -> Option<String> {
    for part in link.split(',') {
        let part = part.trim();
        if !part.contains("rel=\"next\"") && !part.contains("rel='next'") {
            continue;
        }
        let start = part.find('<')? + 1;
        let end = part.find('>')?;
        let url = Url::parse(part.get(start..end)?).ok()?;
        let parameter = |key: &str| {
            url.query_pairs()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.into_owned())
        };
        if let Some(cursor) = parameter("cursor") {
            return Some(cursor);
        }
        if let Some(offset) = parameter("offset") {
            return Some(format!("{OFFSET_PREFIX}{offset}"));
        }
    }
    None
}

fn model_id(model: &HuggingFaceModel) -> String {
    model
        .model_id
        .clone()
        .or_else(|| model.id.clone())
        .unwrap_or_default()
}

fn to_model(model: HuggingFaceModel) -> ModelEntry {
    let id = model_id(&model);
    let tags = model.tags.clone().unwrap_or_default();
    let pipeline = model.pipeline_tag.clone().or_else(|| {
        model
            .card_data
            .as_ref()
            .and_then(|card| card.pipeline_tag.clone())
    });
    let family = model
        .gguf
        .as_ref()
        .and_then(|gguf| gguf.architecture.clone())
        .or_else(|| {
            tags.iter()
                .find_map(|tag| extract_model_family(tag))
                .or_else(|| extract_model_family(&id))
        });
    let parameter_size = extract_parameter_size(&id).or_else(|| {
        tags.iter().find_map(|tag| {
            if is_parameter_size_chip(tag) {
                Some(tag.to_uppercase())
            } else {
                extract_parameter_size(tag)
            }
        })
    });
    let declared_size = model
        .gguf
        .as_ref()
        .and_then(|gguf| gguf.total_file_size.or(gguf.total));
    let context_length = model.gguf.as_ref().and_then(|gguf| gguf.context_length);
    let license = model
        .card_data
        .as_ref()
        .and_then(|card| card.license.clone());
    let author = model
        .author
        .clone()
        .or_else(|| id.split_once('/').map(|(owner, _)| owner.to_string()));
    let description = describe(&model, pipeline.as_deref(), family.as_deref());
    let format = weight_format(&model, &tags);
    let siblings = model.siblings.as_deref().unwrap_or(&[]);
    let size = if format == "gguf" {
        declared_size
    } else {
        safetensors_size(siblings)
    };
    let sizes = if format == "gguf" {
        gguf_variants(&id, siblings)
    } else {
        safetensors_variants(&id, siblings)
    };
    let use_cases = nonempty_vec(use_cases_from_pipeline(pipeline.as_deref(), &tags));
    let mut capabilities = declared_capabilities(
        pipeline
            .as_deref()
            .into_iter()
            .chain(tags.iter().map(String::as_str)),
    );
    if format == "lora" {
        capabilities = Some(vec![
            ModelCapability::ImageGeneration,
            ModelCapability::ImageInput,
        ]);
    }
    let public_tags = nonempty_vec(
        tags.into_iter()
            .filter(|tag| {
                let lowered = tag.to_lowercase();
                !lowered.starts_with("arxiv:")
                    && !lowered.starts_with("base_model:")
                    && !lowered.starts_with("license:")
                    && !lowered.starts_with("region:")
                    && lowered != "endpoints_compatible"
                    && lowered != "transformers"
            })
            .take(8)
            .collect(),
    );

    ModelEntry {
        name: id.clone(),
        size,
        digest: model.sha,
        modified_at: model.last_modified.or(model.created_at),
        description,
        author,
        url: Some(format!("https://huggingface.co/{id}")),
        downloads: model.downloads,
        likes: model.likes,
        tags: public_tags,
        use_cases,
        capabilities,
        sizes,
        details: Some(ModelDetails {
            format: Some(format),
            family,
            parameter_size,
            context_length,
            license,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn weight_format(model: &HuggingFaceModel, tags: &[String]) -> String {
    if model.gguf.is_some() {
        return "gguf".to_string();
    }
    let tagged = tags.iter().any(|tag| {
        let lowered = tag.to_lowercase();
        lowered.contains("lora")
            || lowered == "peft"
            || lowered.starts_with("base_model:adapter:")
            || lowered.starts_with("adapter:")
    });
    if tagged {
        "lora".to_string()
    } else {
        "checkpoint".to_string()
    }
}

fn describe(
    model: &HuggingFaceModel,
    pipeline: Option<&str>,
    family: Option<&str>,
) -> Option<String> {
    let mut sentence = match pipeline {
        Some(tag) => format!("{} GGUF model", humanize_label(tag)),
        None => "GGUF model".to_string(),
    };
    if let Some(author) = model.author.as_deref() {
        sentence.push_str(" by ");
        sentence.push_str(author);
    }
    if let Some(family) = family {
        sentence.push_str(". ");
        sentence.push_str(&humanize_label(family));
        sentence.push_str(" architecture");
    }
    if let Some(context) = model.gguf.as_ref().and_then(|gguf| gguf.context_length) {
        sentence.push_str(" with ");
        sentence.push_str(&format_context_tokens(context));
        sentence.push_str(" context");
    }
    sentence.push('.');
    Some(sentence)
}

fn is_safetensors_weight(filename: &str) -> bool {
    let name = filename.to_ascii_lowercase();
    name.ends_with(".safetensors") && !name.contains("mmproj")
}

fn safetensors_size(siblings: &[HuggingFaceSibling]) -> Option<u64> {
    siblings
        .iter()
        .filter(|sibling| is_safetensors_weight(&sibling.rfilename))
        .filter_map(|sibling| sibling.size)
        .max()
}

fn safetensors_variants(id: &str, siblings: &[HuggingFaceSibling]) -> Option<Vec<ModelSize>> {
    let mut sizes: Vec<ModelSize> = siblings
        .iter()
        .filter(|sibling| is_safetensors_weight(&sibling.rfilename))
        .map(|sibling| {
            let filename = sibling
                .rfilename
                .rsplit('/')
                .next()
                .unwrap_or(&sibling.rfilename);
            ModelSize {
                name: format!("{id}:{filename}"),
                label: filename.to_string(),
                size: sibling.size,
            }
        })
        .collect();
    sizes.sort_by(|left, right| left.label.cmp(&right.label));
    sizes.dedup_by(|left, right| left.name == right.name);
    nonempty(sizes)
}

fn is_gguf_weight_file(filename: &str) -> bool {
    let name = filename
        .rsplit('/')
        .next()
        .unwrap_or(filename)
        .to_lowercase();
    name.ends_with(".gguf") && !name.contains("mmproj") && !name.contains("imatrix")
}

fn gguf_file_stem(filename: &str) -> String {
    let name = filename.rsplit('/').next().unwrap_or(filename);
    let stem = name
        .strip_suffix(".gguf")
        .or_else(|| name.strip_suffix(".GGUF"))
        .unwrap_or(name);
    GGUF_SHARD_PATTERN.replace(stem, "").into_owned()
}

fn filename_parameter_size(filename: &str) -> Option<String> {
    extract_all_parameter_sizes(filename)
        .into_iter()
        .next()
        .or_else(|| extract_parameter_size(filename).filter(|label| !label.contains('·')))
}

/// Distinct GGUF quantizations in a repository, each a separate download.
fn gguf_variants(id: &str, siblings: &[HuggingFaceSibling]) -> Option<Vec<ModelSize>> {
    let mut files: Vec<GgufVariant> = Vec::new();
    for sibling in siblings {
        if !is_gguf_weight_file(&sibling.rfilename) {
            continue;
        }
        let stem = gguf_file_stem(&sibling.rfilename);
        if let Some(existing) = files.iter_mut().find(|file| file.stem == stem) {
            existing.size = match (existing.size, sibling.size) {
                (Some(left), Some(right)) => Some(left + right),
                (left, right) => left.or(right),
            };
            continue;
        }
        files.push(GgufVariant {
            stem,
            quantization: extract_quantization(&sibling.rfilename),
            parameter_size: filename_parameter_size(&sibling.rfilename),
            size: sibling.size,
        });
    }

    if files.len() < 2 {
        return None;
    }

    let distinct: HashSet<&str> = files
        .iter()
        .filter_map(|file| file.parameter_size.as_deref())
        .collect();
    let needs_parameter = distinct.len() > 1;

    let mut sizes: Vec<ModelSize> = files
        .iter()
        .map(|file| {
            let shared = file
                .quantization
                .as_deref()
                .map(|quantization| {
                    files
                        .iter()
                        .filter(|other| other.quantization.as_deref() == Some(quantization))
                        .count()
                })
                .unwrap_or(0);
            let tag = match &file.quantization {
                Some(quantization) if shared == 1 => quantization.clone(),
                Some(quantization) if needs_parameter => match &file.parameter_size {
                    Some(parameter) => format!("{parameter}-{quantization}"),
                    None => file.stem.clone(),
                },
                _ => file.stem.clone(),
            };
            let label = match (needs_parameter, &file.parameter_size, &file.quantization) {
                (true, Some(parameter), Some(quantization)) => {
                    format!("{parameter} · {quantization}")
                }
                (_, _, Some(quantization)) => quantization.clone(),
                _ => file.stem.clone(),
            };
            ModelSize {
                name: format!("{id}:{tag}"),
                label,
                size: file.size,
            }
        })
        .collect();

    sizes.sort_by(compare_downloads);
    Some(sizes)
}

fn compare_downloads(left: &ModelSize, right: &ModelSize) -> Ordering {
    compare(
        download_parameter_billions(&left.label),
        download_parameter_billions(&right.label),
    )
    .then_with(|| {
        compare(
            quantization_bit_width(&left.label),
            quantization_bit_width(&right.label),
        )
    })
    .then_with(|| quantization_preference(&left.label).cmp(&quantization_preference(&right.label)))
    .then_with(|| left.label.cmp(&right.label))
}

fn nonempty(sizes: Vec<ModelSize>) -> Option<Vec<ModelSize>> {
    (!sizes.is_empty()).then_some(sizes)
}

async fn fetch_adapter_page(
    catalog_url: &str,
    client: &Client,
    query: Option<&str>,
    base: &str,
    limit: usize,
) -> Result<Vec<ModelEntry>, CatalogError> {
    let mut url = catalogue_url(catalog_url)?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs
            .append_pair("filter", &format!("base_model:adapter:{base}"))
            .append_pair("sort", "downloads")
            .append_pair("direction", "-1")
            .append_pair("limit", &limit.to_string());
        for field in EXPANDED_FIELDS.iter().filter(|field| **field != "gguf") {
            pairs.append_pair(EXPAND_PARAMETER, field);
        }
        if let Some(query) = query.map(str::trim).filter(|value| !value.is_empty()) {
            pairs.append_pair("search", query);
        }
    }
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(CatalogError::Unavailable(format!(
            "HuggingFace API returned status: {}",
            response.status()
        )));
    }
    let body = response.text().await?;
    let models: Vec<HuggingFaceModel> =
        serde_json::from_str(&body).map_err(|error| CatalogError::Parse(error.to_string()))?;
    Ok(models
        .into_iter()
        .filter(|model| adapter_matches_base(model, base))
        .map(|model| {
            let mut mapped = to_model(model);
            if format_of(&mapped) == Some("gguf") {
                return mapped;
            }
            if let Some(details) = mapped.details.as_mut() {
                details.format = Some("lora".to_string());
            }
            mapped
        })
        .filter(|model| format_of(model) == Some("lora"))
        .collect())
}

fn format_of(model: &ModelEntry) -> Option<&str> {
    model
        .details
        .as_ref()
        .and_then(|details| details.format.as_deref())
}

fn adapter_matches_base(model: &HuggingFaceModel, base: &str) -> bool {
    let expected = format!("base_model:adapter:{base}");
    let tags = model.tags.as_deref().unwrap_or(&[]);
    if tags.iter().any(|tag| tag.eq_ignore_ascii_case(&expected)) {
        return true;
    }
    match model
        .card_data
        .as_ref()
        .and_then(|card| card.base_model.as_ref())
    {
        Some(serde_json::Value::String(value)) => value.eq_ignore_ascii_case(base),
        Some(serde_json::Value::Array(values)) => values.iter().any(|value| {
            value
                .as_str()
                .is_some_and(|value| value.eq_ignore_ascii_case(base))
        }),
        _ => false,
    }
}

/// The hub origin a catalogue URL belongs to, for building repository links.
pub fn huggingface_hub_origin(catalog_url: &str) -> String {
    catalog_url
        .trim_end_matches('/')
        .strip_suffix("/api/models")
        .unwrap_or(catalog_url)
        .trim_end_matches('/')
        .to_string()
}

/// Strip `hf.co/` and any `:tag` so a pull name maps back to a repository.
pub fn huggingface_repo_id(name: &str) -> Option<&str> {
    let name = name
        .strip_prefix("hf.co/")
        .or_else(|| name.strip_prefix("huggingface.co/"))
        .unwrap_or(name);
    let repository = name.split_once(':').map(|(repo, _)| repo).unwrap_or(name);
    repository.contains('/').then_some(repository)
}

/// Read one repository with its blob sizes, for the download list of a model
/// that is not installed locally.
pub async fn huggingface_repo_downloads(
    catalog_url: &str,
    proxy_url: Option<&str>,
    repo_id: &str,
) -> Result<ModelEntry, CatalogError> {
    let client = build_client(proxy_url)?;
    let mut url = catalogue_url(catalog_url)?;
    url.path_segments_mut()
        .map_err(|()| CatalogError::InvalidUrl(catalog_url.to_string()))?
        .pop_if_empty()
        .extend(repo_id.split('/'));
    url.query_pairs_mut()
        .append_pair("blobs", "true")
        .append_pair(EXPAND_PARAMETER, "gguf")
        .append_pair(EXPAND_PARAMETER, "siblings");
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(CatalogError::Unavailable(format!(
            "HuggingFace API returned status: {}",
            response.status()
        )));
    }
    let parsed: HuggingFaceModel = response.json().await.map_err(|error| {
        CatalogError::Parse(format!(
            "Failed to parse HuggingFace model: {}",
            error.without_url()
        ))
    })?;
    Ok(to_model(parsed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    fn browse<'a>(
        sort: ModelSort,
        family: Option<&'a str>,
        size: ModelSizeFilter,
    ) -> BrowseQuery<'a> {
        browse_with_medium(sort, family, size, ModelMediumFilter::All)
    }

    fn browse_with_medium<'a>(
        sort: ModelSort,
        family: Option<&'a str>,
        size: ModelSizeFilter,
        medium: ModelMediumFilter,
    ) -> BrowseQuery<'a> {
        BrowseQuery {
            query: None,
            cursor: None,
            limit: 20,
            sort,
            family,
            size,
            medium,
        }
    }

    fn sibling(name: &str, size: Option<u64>) -> HuggingFaceSibling {
        HuggingFaceSibling {
            rfilename: name.to_string(),
            size,
        }
    }

    fn pairs(url: &Url) -> Vec<(String, String)> {
        url.query_pairs()
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect()
    }

    fn values(url: &Url, key: &str) -> Vec<String> {
        pairs(url)
            .into_iter()
            .filter(|(name, _)| name == key)
            .map(|(_, value)| value)
            .collect()
    }

    #[test]
    fn a_cursor_holding_query_syntax_stays_one_parameter() {
        let options = browse(ModelSort::Relevance, None, ModelSizeFilter::All);

        let url = search_url(
            DEFAULT_HUGGINGFACE_MODELS_URL,
            &options,
            Some("abc&limit=500&filter=secret"),
            20,
        )
        .unwrap();

        assert_eq!(values(&url, "cursor"), vec!["abc&limit=500&filter=secret"]);
        assert_eq!(values(&url, "limit"), vec!["20"]);
        assert_eq!(values(&url, "filter"), vec!["gguf"]);
    }

    #[test]
    fn a_bare_number_cursor_pages_by_offset() {
        let options = browse(ModelSort::Relevance, None, ModelSizeFilter::All);

        let url = search_url(DEFAULT_HUGGINGFACE_MODELS_URL, &options, Some("40"), 20).unwrap();

        assert_eq!(values(&url, "offset"), vec!["40"]);
        assert!(values(&url, "cursor").is_empty());
    }

    #[test]
    fn a_malformed_offset_cursor_is_refused() {
        let options = browse(ModelSort::Relevance, None, ModelSizeFilter::All);
        assert!(
            search_url(
                DEFAULT_HUGGINGFACE_MODELS_URL,
                &options,
                Some("offset:x"),
                20
            )
            .is_err()
        );
    }

    #[test]
    fn a_cursor_from_the_link_header_is_decoded_then_encoded_once() {
        let cursor = extract_cursor_from_link_header(
            r#"<https://huggingface.co/api/models?cursor=eyJ2IjoxfQ%3D%3D&limit=20>; rel="next""#,
        )
        .unwrap();
        assert_eq!(cursor, "eyJ2IjoxfQ==");

        let options = browse(ModelSort::Relevance, None, ModelSizeFilter::All);
        let url = search_url(DEFAULT_HUGGINGFACE_MODELS_URL, &options, Some(&cursor), 20).unwrap();
        assert_eq!(values(&url, "cursor"), vec!["eyJ2IjoxfQ=="]);
        assert!(url.as_str().contains("cursor=eyJ2IjoxfQ%3D%3D"), "{url}");
    }

    #[test]
    fn search_url_does_not_filter_by_family_tag() {
        let family_only = search_url(
            DEFAULT_HUGGINGFACE_MODELS_URL,
            &BrowseQuery {
                query: None,
                cursor: None,
                limit: 20,
                sort: ModelSort::Relevance,
                family: Some("qwen"),
                size: ModelSizeFilter::All,
                medium: ModelMediumFilter::All,
            },
            None,
            20,
        )
        .unwrap();
        assert_eq!(values(&family_only, "filter"), vec!["gguf"]);
        assert_eq!(values(&family_only, "search"), vec!["qwen"]);

        let with_query = search_url(
            DEFAULT_HUGGINGFACE_MODELS_URL,
            &BrowseQuery {
                query: Some("coder"),
                cursor: None,
                limit: 20,
                sort: ModelSort::Relevance,
                family: Some("qwen"),
                size: ModelSizeFilter::All,
                medium: ModelMediumFilter::All,
            },
            None,
            20,
        )
        .unwrap();
        assert_eq!(values(&with_query, "filter"), vec!["gguf"]);
        assert_eq!(values(&with_query, "search"), vec!["coder"]);
    }

    #[test]
    fn search_url_passes_both_cursor_styles() {
        let options = browse(ModelSort::Relevance, None, ModelSizeFilter::All);
        let cursor =
            search_url(DEFAULT_HUGGINGFACE_MODELS_URL, &options, Some("abc123"), 20).unwrap();
        assert_eq!(values(&cursor, "cursor"), vec!["abc123"]);

        let offset = search_url(
            DEFAULT_HUGGINGFACE_MODELS_URL,
            &options,
            Some("offset:40"),
            20,
        )
        .unwrap();
        assert_eq!(values(&offset, "offset"), vec!["40"]);
        assert!(values(&offset, "cursor").is_empty());
    }

    #[test]
    fn sort_parameters_map_to_the_api() {
        assert_eq!(sort_parameters(ModelSort::Relevance), ("downloads", -1));
        assert_eq!(
            sort_parameters(ModelSort::UpdatedDescending),
            ("lastModified", -1)
        );
        assert_eq!(
            sort_parameters(ModelSort::UpdatedAscending),
            ("lastModified", 1)
        );
        assert_eq!(
            sort_parameters(ModelSort::DownloadsDescending),
            ("downloads", -1)
        );
        assert_eq!(
            sort_parameters(ModelSort::DownloadsAscending),
            ("downloads", 1)
        );
    }

    #[test]
    fn local_window_covers_filters_and_local_sorts() {
        assert!(!uses_local_window(&browse(
            ModelSort::Relevance,
            None,
            ModelSizeFilter::All
        )));
        assert!(!uses_local_window(&browse(
            ModelSort::UpdatedDescending,
            Some("llama"),
            ModelSizeFilter::All
        )));
        assert!(uses_local_window(&browse(
            ModelSort::NameAscending,
            None,
            ModelSizeFilter::All
        )));
        assert!(uses_local_window(&browse(
            ModelSort::SizeDescending,
            None,
            ModelSizeFilter::All
        )));
        assert!(uses_local_window(&browse(
            ModelSort::Relevance,
            None,
            ModelSizeFilter::Small
        )));
        assert!(uses_local_window(&browse_with_medium(
            ModelSort::Relevance,
            None,
            ModelSizeFilter::All,
            ModelMediumFilter::Image
        )));
        assert!(!uses_local_window(&browse_with_medium(
            ModelSort::Relevance,
            None,
            ModelSizeFilter::All,
            ModelMediumFilter::All
        )));
        assert_eq!(
            medium_tag(ModelMediumFilter::Image),
            Some("image-text-to-text")
        );
        assert_eq!(
            medium_tag(ModelMediumFilter::Embeddings),
            Some("feature-extraction")
        );
        assert_eq!(medium_tag(ModelMediumFilter::Text), None);
        assert!(uses_local_sort(ModelSort::ParametersDescending));
        assert!(!uses_local_sort(ModelSort::UpdatedDescending));
        assert!(!uses_local_sort(ModelSort::DownloadsDescending));
        assert!(!uses_local_sort(ModelSort::DownloadsAscending));
        assert_eq!(window_pages(true), WINDOW_PAGES);
        assert_eq!(window_pages(false), FILTER_MAXIMUM_PAGES);
    }

    #[test]
    fn window_next_cursor_stays_inside_the_window() {
        assert_eq!(
            window_next_cursor(Some("offset:20".into()), true, true, true, 0, 20),
            Some("offset:20".into())
        );
        assert_eq!(window_next_cursor(None, true, true, true, 480, 20), None);
        assert_eq!(
            window_next_cursor(None, true, false, false, 0, 20),
            Some("offset:20".into())
        );
        assert_eq!(window_next_cursor(None, true, false, true, 0, 20), None);
    }

    #[test]
    fn an_empty_window_page_has_no_next_cursor() {
        assert_eq!(window_next_cursor(None, true, false, false, 40, 0), None);
    }

    #[test]
    fn hub_origin_strips_the_models_api() {
        assert_eq!(
            huggingface_hub_origin("https://huggingface.co/api/models"),
            "https://huggingface.co"
        );
        assert_eq!(
            huggingface_hub_origin("http://127.0.0.1:9/api/models"),
            "http://127.0.0.1:9"
        );
    }

    #[test]
    fn repo_id_strips_hosts_and_tags() {
        assert_eq!(
            huggingface_repo_id("TheBloke/Mistral-7B-Instruct-v0.2-GGUF"),
            Some("TheBloke/Mistral-7B-Instruct-v0.2-GGUF")
        );
        assert_eq!(
            huggingface_repo_id("hf.co/TheBloke/Mistral-7B-Instruct-v0.2-GGUF:Q4_0"),
            Some("TheBloke/Mistral-7B-Instruct-v0.2-GGUF")
        );
        assert_eq!(
            huggingface_repo_id("huggingface.co/owner/model"),
            Some("owner/model")
        );
        assert_eq!(huggingface_repo_id("llama3.2:3b"), None);
    }

    #[test]
    fn adapter_maps_safetensors_not_gguf() {
        let model = HuggingFaceModel {
            id: Some("owner/qwen-edit-lora".into()),
            model_id: Some("owner/qwen-edit-lora".into()),
            sha: None,
            last_modified: None,
            created_at: None,
            tags: Some(vec![
                "lora".into(),
                "base_model:adapter:Qwen/Qwen-Image-Edit-2511".into(),
            ]),
            downloads: Some(12),
            likes: Some(1),
            author: Some("owner".into()),
            pipeline_tag: Some("image-to-image".into()),
            card_data: None,
            gguf: None,
            siblings: Some(vec![sibling(
                "qwen-image-edit-plus-nsfw-lora.safetensors",
                Some(563_000_000),
            )]),
        };
        let mapped = to_model(model);
        assert_eq!(format_of(&mapped), Some("lora"));
        assert_eq!(mapped.size, Some(563_000_000));
        assert_eq!(
            mapped.sizes.as_ref().unwrap()[0].name,
            "owner/qwen-edit-lora:qwen-image-edit-plus-nsfw-lora.safetensors"
        );
        assert_eq!(
            mapped.capabilities,
            Some(vec![
                ModelCapability::ImageGeneration,
                ModelCapability::ImageInput
            ])
        );
    }

    #[test]
    fn gguf_variants_are_separate_quantizations() {
        let sizes = gguf_variants(
            "TheBloke/Mistral-7B-Instruct-v0.2-GGUF",
            &[
                sibling("README.md", Some(100)),
                sibling("mistral-7b-instruct-v0.2.Q4_0.gguf", Some(4_108_917_024)),
                sibling("mistral-7b-instruct-v0.2.Q5_K_M.gguf", Some(5_131_409_696)),
                sibling("mistral-7b-instruct-v0.2.Q8_0.gguf", Some(7_695_857_952)),
                sibling("mmproj-model-f16.gguf", Some(600_000_000)),
            ],
        )
        .unwrap();

        assert_eq!(sizes.len(), 3);
        assert_eq!(sizes[0].name, "TheBloke/Mistral-7B-Instruct-v0.2-GGUF:Q4_0");
        assert_eq!(sizes[0].label, "Q4_0");
        assert_eq!(sizes[0].size, Some(4_108_917_024));
        assert_eq!(sizes[1].label, "Q5_K_M");
        assert_eq!(sizes[2].label, "Q8_0");
    }

    #[test]
    fn gguf_variants_skip_a_single_file() {
        assert!(gguf_variants("owner/model", &[sibling("model-Q4_0.gguf", Some(100))]).is_none());
    }

    #[test]
    fn gguf_variants_merge_shards_and_disambiguate() {
        let sizes = gguf_variants(
            "Qwen/Qwen3-GGUF",
            &[
                sibling("Qwen3-0.6B-Q4_K_M-00001-of-00002.gguf", Some(200)),
                sibling("Qwen3-0.6B-Q4_K_M-00002-of-00002.gguf", Some(150)),
                sibling("Qwen3-8B-Q4_K_M.gguf", Some(4_700_000_000)),
                sibling("Qwen3-8B-Q8_0.gguf", Some(8_500_000_000)),
            ],
        )
        .unwrap();

        assert_eq!(sizes.len(), 3);
        assert_eq!(sizes[0].label, "0.6B · Q4_K_M");
        assert_eq!(sizes[0].name, "Qwen/Qwen3-GGUF:0.6B-Q4_K_M");
        assert_eq!(sizes[0].size, Some(350));
        assert_eq!(sizes[1].label, "8B · Q4_K_M");
        assert_eq!(sizes[2].label, "8B · Q8_0");
    }

    #[test]
    fn to_model_exposes_gguf_downloads() {
        let json = r#"{
            "id": "TheBloke/Mistral-7B-Instruct-v0.2-GGUF",
            "modelId": "TheBloke/Mistral-7B-Instruct-v0.2-GGUF",
            "gguf": {"totalFileSize": 3083098400, "architecture": "llama"},
            "siblings": [
                {"rfilename": "mistral-7b-instruct-v0.2.Q4_0.gguf", "size": 4108917024},
                {"rfilename": "mistral-7b-instruct-v0.2.Q5_K_M.gguf", "size": 5131409696},
                {"rfilename": "mistral-7b-instruct-v0.2.Q8_0.gguf", "size": 7695857952}
            ]
        }"#;
        let parsed: HuggingFaceModel = serde_json::from_str(json).unwrap();
        let sizes = to_model(parsed).sizes.expect("gguf downloads");
        assert_eq!(sizes.len(), 3);
        assert_eq!(sizes[0].label, "Q4_0");
        assert_eq!(sizes[1].label, "Q5_K_M");
        assert_eq!(sizes[2].label, "Q8_0");
    }

    #[test]
    fn to_model_includes_catalog_metadata() {
        let json = r#"{
            "id": "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF",
            "modelId": "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF",
            "author": "unsloth",
            "likes": 12,
            "downloads": 1000,
            "pipeline_tag": "text-generation",
            "tags": ["gguf", "qwen", "text-generation", "conversational"],
            "gguf": {"totalFileSize": 17000000000, "architecture": "qwen3moe", "context_length": 262144},
            "cardData": {"license": "apache-2.0", "pipeline_tag": "text-generation"}
        }"#;
        let parsed: HuggingFaceModel = serde_json::from_str(json).unwrap();
        let model = to_model(parsed);
        let details = model.details.as_ref().expect("details");
        assert_eq!(model.size, Some(17_000_000_000));
        assert_eq!(details.parameter_size.as_deref(), Some("30B"));
        assert_eq!(details.context_length, Some(262_144));
        assert_eq!(details.license.as_deref(), Some("apache-2.0"));
        assert!(
            model
                .description
                .as_ref()
                .unwrap()
                .contains("Text Generation")
        );
        assert!(
            model
                .use_cases
                .as_ref()
                .unwrap()
                .contains(&"Chat".to_string())
        );
        assert_eq!(model.author.as_deref(), Some("unsloth"));
    }

    #[test]
    fn capabilities_are_declared_tasks_only() {
        let parsed = serde_json::from_value(serde_json::json!({
            "id": "tools-vision/audio-reasoning",
            "pipeline_tag": "text-generation",
            "tags": ["not-tools", "image-generation-guide", "gguf"],
            "description": "image generation tools audio reasoning"
        }))
        .unwrap();
        assert_eq!(
            to_model(parsed).capabilities,
            Some(vec![ModelCapability::Text])
        );
    }

    #[test]
    fn json_parsing_prefers_model_id() {
        let json = r#"[
            {"_id":"123","id":"author/model","modelId":"author/model","likes":100,"downloads":1000,"tags":["gguf"]},
            {"_id":"456","id":"other/model2","modelId":"other/model2","likes":50,"downloads":500}
        ]"#;

        let models: Vec<HuggingFaceModel> = serde_json::from_str(json).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(model_id(&models[0]), "author/model");
        assert_eq!(model_id(&models[1]), "other/model2");
    }

    #[test]
    fn cursor_comes_from_the_next_link() {
        assert_eq!(
            extract_cursor_from_link_header(
                r#"<https://huggingface.co/api/models?cursor=abc123>; rel="next""#
            ),
            Some("abc123".to_string())
        );
        assert_eq!(
            extract_cursor_from_link_header(
                r#"<https://huggingface.co/api/models?cursor=xyz789&limit=20>; rel="next""#
            ),
            Some("xyz789".to_string())
        );
        assert_eq!(extract_cursor_from_link_header(""), None);
    }

    #[test]
    fn cursor_falls_back_to_an_offset_link() {
        assert_eq!(
            extract_cursor_from_link_header(
                r#"<https://huggingface.co/api/models?filter=gguf&sort=downloads&limit=20&offset=20>; rel="next""#
            ),
            Some("offset:20".to_string())
        );
        assert_eq!(
            extract_cursor_from_link_header(
                r#"<https://huggingface.co/api/models?offset=0>; rel="prev""#
            ),
            None
        );
    }

    #[tokio::test]
    async fn search_maps_a_catalog_page() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/models"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{
                        "id": "Qwen/Qwen2.5-Coder-7B-Instruct-GGUF",
                        "modelId": "Qwen/Qwen2.5-Coder-7B-Instruct-GGUF",
                        "downloads": 1234,
                        "pipeline_tag": "text-generation",
                        "gguf": {"totalFileSize": 4_700_000_000_u64, "architecture": "qwen2"},
                        "siblings": [
                            {"rfilename": "qwen2.5-coder-7b-q4_k_m.gguf", "size": 4_700_000_000_u64},
                            {"rfilename": "qwen2.5-coder-7b-q8_0.gguf", "size": 8_100_000_000_u64}
                        ]
                    }]))
                    .insert_header(
                        "link",
                        r#"<https://huggingface.co/api/models?cursor=next123>; rel="next""#,
                    ),
            )
            .mount(&server)
            .await;

        let provider = HuggingFaceProvider::new(format!("{}/api/models", server.uri())).unwrap();
        let page = provider
            .search(browse(ModelSort::Relevance, None, ModelSizeFilter::All))
            .await
            .unwrap();

        assert_eq!(page.models.len(), 1);
        assert_eq!(page.models[0].name, "Qwen/Qwen2.5-Coder-7B-Instruct-GGUF");
        assert_eq!(
            page.models[0]
                .details
                .as_ref()
                .unwrap()
                .parameter_size
                .as_deref(),
            Some("7B")
        );
        assert_eq!(page.next_cursor.as_deref(), Some("next123"));
    }

    #[tokio::test]
    async fn search_skips_the_network_for_image_generation() {
        let provider = HuggingFaceProvider::new("http://catalog.invalid/api/models").unwrap();
        let page = provider
            .search(browse_with_medium(
                ModelSort::Relevance,
                None,
                ModelSizeFilter::All,
                ModelMediumFilter::ImageGeneration,
            ))
            .await
            .unwrap();
        assert_eq!(page, ModelPage::default());
    }

    #[tokio::test]
    async fn search_reports_an_unavailable_catalog() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let provider = HuggingFaceProvider::new(format!("{}/api/models", server.uri())).unwrap();
        let error = provider
            .search(browse(ModelSort::Relevance, None, ModelSizeFilter::All))
            .await
            .unwrap_err();
        assert!(matches!(error, CatalogError::Unavailable(_)));
    }

    #[tokio::test]
    async fn search_reports_a_malformed_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let provider = HuggingFaceProvider::new(format!("{}/api/models", server.uri())).unwrap();
        let error = provider
            .search(browse(ModelSort::Relevance, None, ModelSizeFilter::All))
            .await
            .unwrap_err();
        assert!(matches!(error, CatalogError::Parse(_)));
    }

    #[tokio::test]
    async fn search_adapters_keeps_only_adapters_of_the_base() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": "owner/edit-lora",
                    "modelId": "owner/edit-lora",
                    "downloads": 40,
                    "tags": ["lora", "base_model:adapter:Qwen/Qwen-Image-Edit"],
                    "siblings": [{"rfilename": "edit.safetensors", "size": 1_000_u64}]
                },
                {
                    "id": "owner/unrelated",
                    "modelId": "owner/unrelated",
                    "downloads": 90,
                    "tags": ["lora"]
                }
            ])))
            .mount(&server)
            .await;

        let provider = HuggingFaceProvider::new(format!("{}/api/models", server.uri())).unwrap();
        let adapters = provider
            .search_adapters(
                browse(ModelSort::Relevance, None, ModelSizeFilter::All),
                &["Qwen/Qwen-Image-Edit".to_string()],
            )
            .await
            .unwrap();

        assert_eq!(adapters.len(), 1);
        assert_eq!(adapters[0].name, "owner/edit-lora");
        assert_eq!(format_of(&adapters[0]), Some("lora"));
    }

    #[tokio::test]
    async fn search_adapters_without_bases_makes_no_request() {
        let provider = HuggingFaceProvider::new("http://catalog.invalid/api/models").unwrap();
        let adapters = provider
            .search_adapters(
                browse(ModelSort::Relevance, None, ModelSizeFilter::All),
                &[],
            )
            .await
            .unwrap();
        assert!(adapters.is_empty());
    }

    #[tokio::test]
    async fn repo_downloads_reads_file_sizes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/api/models/TheBloke/Mistral-7B-Instruct-v0.2-GGUF",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "TheBloke/Mistral-7B-Instruct-v0.2-GGUF",
                "modelId": "TheBloke/Mistral-7B-Instruct-v0.2-GGUF",
                "gguf": {"totalFileSize": 4_108_917_024_u64, "architecture": "llama"},
                "siblings": [
                    {"rfilename": "mistral-7b-instruct-v0.2.Q4_0.gguf", "size": 4_108_917_024_u64},
                    {"rfilename": "mistral-7b-instruct-v0.2.Q5_K_M.gguf", "size": 5_131_409_696_u64},
                    {"rfilename": "mistral-7b-instruct-v0.2.Q8_0.gguf", "size": 7_695_857_952_u64}
                ]
            })))
            .mount(&server)
            .await;

        let catalog = format!("{}/api/models", server.uri());
        let model =
            huggingface_repo_downloads(&catalog, None, "TheBloke/Mistral-7B-Instruct-v0.2-GGUF")
                .await
                .unwrap();

        let sizes = model.sizes.expect("gguf downloads");
        assert_eq!(model.size, Some(4_108_917_024));
        assert_eq!(sizes.len(), 3);
        assert_eq!(sizes[0].label, "Q4_0");
        assert_eq!(sizes[1].label, "Q5_K_M");
        assert_eq!(sizes[2].label, "Q8_0");
    }

    #[tokio::test]
    async fn repo_downloads_reports_a_missing_repository() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let catalog = format!("{}/api/models", server.uri());
        let error = huggingface_repo_downloads(&catalog, None, "owner/missing")
            .await
            .unwrap_err();
        assert!(matches!(error, CatalogError::Unavailable(_)));
    }

    #[tokio::test]
    async fn a_repository_that_does_not_parse_is_reported_without_its_url() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let catalog = format!("{}/api/models", server.uri());
        let error = huggingface_repo_downloads(&catalog, None, "owner/repo")
            .await
            .unwrap_err();

        let CatalogError::Parse(message) = &error else {
            panic!("expected a parse failure, got {error:?}");
        };
        assert!(
            !message.contains(&server.address().to_string()),
            "{message}"
        );
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

        let provider = HuggingFaceProvider::new(format!("{}/api/models", server.uri())).unwrap();
        let error = provider
            .search(browse(ModelSort::Relevance, None, ModelSizeFilter::All))
            .await
            .unwrap_err();

        assert!(matches!(error, CatalogError::Parse(_)), "{error:?}");
    }
}
