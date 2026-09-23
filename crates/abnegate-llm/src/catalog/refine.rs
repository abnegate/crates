use crate::catalog::capability::ModelCapability;
use crate::catalog::entry::ModelEntry;
use crate::catalog::error::CatalogError;
use crate::catalog::medium_filter::ModelMediumFilter;
use crate::catalog::page::DEFAULT_PAGE_SIZE;
use crate::catalog::page::ModelPage;
use crate::catalog::parse::parse_param_billions;
use crate::catalog::query::BrowseQuery;
use crate::catalog::size_filter::ModelSizeFilter;
use crate::catalog::sort::ModelSort;
use std::cmp::Ordering;
use std::cmp::Reverse;

pub(crate) fn refine_models(
    mut models: Vec<ModelEntry>,
    options: &BrowseQuery<'_>,
) -> Vec<ModelEntry> {
    models.retain(|model| model_matches_query(model, options));
    sort_models(&mut models, options.sort);
    models
}

pub(crate) fn count_matching_models(models: &[ModelEntry], options: &BrowseQuery<'_>) -> usize {
    models
        .iter()
        .filter(|model| model_matches_query(model, options))
        .count()
}

pub(crate) fn paginate_models(models: Vec<ModelEntry>, offset: usize, limit: usize) -> ModelPage {
    let total = models.len();
    let page: Vec<ModelEntry> = models.into_iter().skip(offset).take(limit).collect();
    let next_offset = offset + page.len();
    let next_cursor = (next_offset < total).then(|| format!("offset:{next_offset}"));

    ModelPage {
        models: page,
        next_cursor,
    }
}

/// Read a browse cursor as an offset.
///
/// `offset:N` and `page:N` come from this crate's own pagination; a bare number
/// is what the HuggingFace Link header carries.
pub(crate) fn parse_cursor_offset(cursor: Option<&str>) -> Result<usize, CatalogError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };

    if let Some(offset) = cursor.strip_prefix("offset:") {
        return offset
            .parse()
            .map_err(|_| CatalogError::Parse("Invalid cursor offset format".into()));
    }

    if let Some(page) = cursor.strip_prefix("page:") {
        let page: usize = page
            .parse()
            .map_err(|_| CatalogError::Parse("Invalid cursor page format".into()))?;
        return Ok(page.saturating_sub(1) * DEFAULT_PAGE_SIZE);
    }

    cursor
        .parse()
        .map_err(|_| CatalogError::Parse(format!("Unknown cursor format: {cursor}")))
}

pub(crate) fn model_matches_query(model: &ModelEntry, options: &BrowseQuery<'_>) -> bool {
    if let Some(family) = options.family
        && !model_matches_family(model, family)
    {
        return false;
    }

    if options.size != ModelSizeFilter::All && !model_matches_size(model, options.size) {
        return false;
    }

    options.medium == ModelMediumFilter::All || model_matches_medium(model, options.medium)
}

pub(crate) fn model_matches_family(model: &ModelEntry, family: &str) -> bool {
    let needle = family.to_lowercase();
    if needle.is_empty() {
        return true;
    }

    if let Some(details) = &model.details
        && let Some(model_family) = &details.family
        && model_family.to_lowercase().contains(&needle)
    {
        return true;
    }

    model.name.to_lowercase().contains(&needle)
}

pub(crate) fn model_matches_size(model: &ModelEntry, size: ModelSizeFilter) -> bool {
    let Some(billions) = param_billions(model) else {
        return false;
    };

    match size {
        ModelSizeFilter::All => true,
        ModelSizeFilter::Small => billions < 4.0,
        ModelSizeFilter::Medium => (4.0..16.0).contains(&billions),
        ModelSizeFilter::Large => (16.0..40.0).contains(&billions),
        ModelSizeFilter::Xl => billions >= 40.0,
    }
}

pub(crate) fn model_matches_medium(model: &ModelEntry, medium: ModelMediumFilter) -> bool {
    use ModelCapability::*;

    let wanted: &[ModelCapability] = match medium {
        ModelMediumFilter::All => return true,
        ModelMediumFilter::Text => {
            return match model.capabilities.as_deref() {
                None | Some([]) => true,
                Some(capabilities) => capabilities.iter().any(|capability| {
                    matches!(
                        capability,
                        Text | ImageInput | VideoInput | Audio | AudioInput | Tools | Reasoning
                    )
                }),
            };
        }
        ModelMediumFilter::Image => &[ImageInput, ImageGeneration],
        ModelMediumFilter::ImageGeneration => &[ImageGeneration],
        ModelMediumFilter::Video => &[VideoInput, VideoGeneration],
        ModelMediumFilter::Audio => &[Audio, AudioInput, AudioGeneration],
        ModelMediumFilter::Tools => &[Tools],
        ModelMediumFilter::Embeddings => &[Embeddings],
        ModelMediumFilter::Reasoning => &[Reasoning],
    };

    model.capabilities.as_deref().is_some_and(|capabilities| {
        capabilities
            .iter()
            .any(|capability| wanted.contains(capability))
    })
}

fn sort_models(models: &mut [ModelEntry], sort: ModelSort) {
    match sort {
        ModelSort::Relevance => {}
        ModelSort::NameAsc => models.sort_by_key(|model| model.name.to_lowercase()),
        ModelSort::NameDesc => models.sort_by_key(|model| Reverse(model.name.to_lowercase())),
        ModelSort::DownloadsAsc => {
            models.sort_by(|left, right| compare(left.downloads, right.downloads))
        }
        ModelSort::DownloadsDesc => {
            models.sort_by(|left, right| compare_desc(left.downloads, right.downloads))
        }
        ModelSort::SizeAsc => models.sort_by(|left, right| compare(left.size, right.size)),
        ModelSort::SizeDesc => models.sort_by(|left, right| compare_desc(left.size, right.size)),
        ModelSort::ParamsAsc => {
            models.sort_by(|left, right| compare(param_billions(left), param_billions(right)))
        }
        ModelSort::ParamsDesc => {
            models.sort_by(|left, right| compare_desc(param_billions(left), param_billions(right)))
        }
        ModelSort::UpdatedAsc => models.sort_by(|left, right| {
            compare(left.modified_at.as_deref(), right.modified_at.as_deref())
        }),
        ModelSort::UpdatedDesc => models.sort_by(|left, right| {
            compare_desc(left.modified_at.as_deref(), right.modified_at.as_deref())
        }),
    }
}

fn param_billions(model: &ModelEntry) -> Option<f64> {
    model
        .details
        .as_ref()
        .and_then(|details| details.parameter_size.as_deref())
        .and_then(parse_param_billions)
}

pub(crate) fn compare<T: PartialOrd>(left: Option<T>, right: Option<T>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.partial_cmp(&right).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn compare_desc<T: PartialOrd>(left: Option<T>, right: Option<T>) -> Ordering {
    if left.is_some() && right.is_some() {
        compare(left, right).reverse()
    } else {
        compare(left, right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::details::ModelDetails;

    fn model(
        name: &str,
        size: Option<u64>,
        family: Option<&str>,
        parameters: Option<&str>,
        modified_at: Option<&str>,
    ) -> ModelEntry {
        ModelEntry {
            name: name.to_string(),
            size,
            modified_at: modified_at.map(ToString::to_string),
            details: Some(ModelDetails {
                format: Some("gguf".to_string()),
                family: family.map(ToString::to_string),
                parameter_size: parameters.map(ToString::to_string),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn model_with_capabilities(
        name: &str,
        capabilities: Option<Vec<ModelCapability>>,
    ) -> ModelEntry {
        ModelEntry {
            capabilities,
            ..model(name, None, None, None, None)
        }
    }

    pub(super) fn browse<'a>(
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

    #[test]
    fn parse_cursor_offset_reads_known_formats() {
        assert_eq!(parse_cursor_offset(None).unwrap(), 0);
        assert_eq!(parse_cursor_offset(Some("offset:20")).unwrap(), 20);
        assert_eq!(parse_cursor_offset(Some("offset:100")).unwrap(), 100);
        assert_eq!(parse_cursor_offset(Some("page:1")).unwrap(), 0);
        assert_eq!(parse_cursor_offset(Some("page:2")).unwrap(), 20);
        assert_eq!(parse_cursor_offset(Some("page:3")).unwrap(), 40);
        assert!(parse_cursor_offset(Some("invalid")).is_err());
        assert!(parse_cursor_offset(Some("offset:abc")).is_err());
        assert!(parse_cursor_offset(Some("page:xyz")).is_err());
    }

    #[test]
    fn parse_cursor_offset_reads_a_bare_number() {
        assert_eq!(parse_cursor_offset(Some("20")).unwrap(), 20);
        assert_eq!(parse_cursor_offset(Some("100")).unwrap(), 100);
        assert!(parse_cursor_offset(Some("abc")).is_err());
    }

    #[test]
    fn model_matches_family_reads_name_and_details() {
        let llama = model("llama3.2", None, Some("llama"), Some("3B"), None);
        let mistral = model("mistral", None, Some("mistral"), Some("7B"), None);
        let code = model("codellama", None, Some("codellama"), Some("7B"), None);

        assert!(model_matches_family(&llama, "llama"));
        assert!(!model_matches_family(&mistral, "llama"));
        assert!(model_matches_family(&code, "code"));
        assert!(model_matches_family(&code, "llama"));
    }

    #[test]
    fn model_matches_size_buckets_parameters() {
        let small = model("tiny", None, Some("llama"), Some("3B"), None);
        let medium = model("mid", None, Some("mistral"), Some("7B"), None);
        let large = model("big", None, Some("qwen"), Some("32B"), None);
        let extra_large = model("huge", None, Some("llama"), Some("70B"), None);
        let unknown = model("mystery", None, Some("llama"), None, None);

        assert!(model_matches_size(&small, ModelSizeFilter::Small));
        assert!(model_matches_size(&medium, ModelSizeFilter::Medium));
        assert!(model_matches_size(&large, ModelSizeFilter::Large));
        assert!(model_matches_size(&extra_large, ModelSizeFilter::Xl));
        assert!(!model_matches_size(&unknown, ModelSizeFilter::Small));
    }

    #[test]
    fn model_matches_medium_reads_capabilities() {
        let undeclared = model_with_capabilities("chat", None);
        let text = model_with_capabilities("text", Some(vec![ModelCapability::Text]));
        let vision = model_with_capabilities(
            "vision",
            Some(vec![ModelCapability::Text, ModelCapability::ImageInput]),
        );
        let image = model_with_capabilities("flux", Some(vec![ModelCapability::ImageGeneration]));
        let video = model_with_capabilities(
            "video",
            Some(vec![
                ModelCapability::Text,
                ModelCapability::VideoGeneration,
            ]),
        );
        let audio = model_with_capabilities("whisper", Some(vec![ModelCapability::AudioInput]));
        let tools = model_with_capabilities("agent", Some(vec![ModelCapability::Tools]));
        let embeddings = model_with_capabilities("embed", Some(vec![ModelCapability::Embeddings]));
        let reasoning = model_with_capabilities("think", Some(vec![ModelCapability::Reasoning]));

        assert!(model_matches_medium(&undeclared, ModelMediumFilter::All));
        assert!(model_matches_medium(&undeclared, ModelMediumFilter::Text));
        assert!(!model_matches_medium(&undeclared, ModelMediumFilter::Image));
        assert!(!model_matches_medium(
            &undeclared,
            ModelMediumFilter::Embeddings
        ));

        assert!(model_matches_medium(&text, ModelMediumFilter::Text));
        assert!(!model_matches_medium(&text, ModelMediumFilter::Image));

        assert!(model_matches_medium(&vision, ModelMediumFilter::Text));
        assert!(model_matches_medium(&vision, ModelMediumFilter::Image));
        assert!(!model_matches_medium(&vision, ModelMediumFilter::Video));

        assert!(!model_matches_medium(&image, ModelMediumFilter::Text));
        assert!(model_matches_medium(&image, ModelMediumFilter::Image));

        assert!(model_matches_medium(&video, ModelMediumFilter::Text));
        assert!(model_matches_medium(&video, ModelMediumFilter::Video));
        assert!(!model_matches_medium(&video, ModelMediumFilter::Image));

        assert!(model_matches_medium(&audio, ModelMediumFilter::Text));
        assert!(model_matches_medium(&audio, ModelMediumFilter::Audio));

        assert!(model_matches_medium(&tools, ModelMediumFilter::Text));
        assert!(model_matches_medium(&tools, ModelMediumFilter::Tools));
        assert!(!model_matches_medium(&tools, ModelMediumFilter::Image));

        assert!(!model_matches_medium(&embeddings, ModelMediumFilter::Text));
        assert!(model_matches_medium(
            &embeddings,
            ModelMediumFilter::Embeddings
        ));

        assert!(model_matches_medium(&reasoning, ModelMediumFilter::Text));
        assert!(model_matches_medium(
            &reasoning,
            ModelMediumFilter::Reasoning
        ));
    }

    #[test]
    fn refine_models_filters_by_medium() {
        let models = vec![
            model_with_capabilities("chat", None),
            model_with_capabilities(
                "llava",
                Some(vec![ModelCapability::Text, ModelCapability::ImageInput]),
            ),
            model_with_capabilities("nomic", Some(vec![ModelCapability::Embeddings])),
            model_with_capabilities("qwen-tools", Some(vec![ModelCapability::Tools])),
        ];

        let text = refine_models(
            models.clone(),
            &browse_with_medium(
                ModelSort::NameAsc,
                None,
                ModelSizeFilter::All,
                ModelMediumFilter::Text,
            ),
        );
        assert_eq!(
            text.iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec!["chat", "llava", "qwen-tools"]
        );

        let image = refine_models(
            models.clone(),
            &browse_with_medium(
                ModelSort::Relevance,
                None,
                ModelSizeFilter::All,
                ModelMediumFilter::Image,
            ),
        );
        assert_eq!(image.len(), 1);
        assert_eq!(image[0].name, "llava");

        let embeddings = refine_models(
            models,
            &browse_with_medium(
                ModelSort::Relevance,
                None,
                ModelSizeFilter::All,
                ModelMediumFilter::Embeddings,
            ),
        );
        assert_eq!(embeddings.len(), 1);
        assert_eq!(embeddings[0].name, "nomic");
    }

    #[test]
    fn refine_models_filters_and_sorts() {
        let models = vec![
            model(
                "mistral",
                Some(20),
                Some("mistral"),
                Some("7B"),
                Some("2024-01-01"),
            ),
            model(
                "llama-70b",
                Some(40),
                Some("llama"),
                Some("70B"),
                Some("2024-03-01"),
            ),
            model(
                "llama-3b",
                Some(10),
                Some("llama"),
                Some("3B"),
                Some("2024-02-01"),
            ),
        ];

        let filtered = refine_models(
            models.clone(),
            &browse(ModelSort::NameAsc, Some("llama"), ModelSizeFilter::All),
        );
        assert_eq!(
            filtered
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec!["llama-3b", "llama-70b"]
        );

        let small = refine_models(
            models.clone(),
            &browse(ModelSort::Relevance, Some("llama"), ModelSizeFilter::Small),
        );
        assert_eq!(small.len(), 1);
        assert_eq!(small[0].name, "llama-3b");

        let by_parameters = refine_models(
            models,
            &browse(ModelSort::ParamsDesc, None, ModelSizeFilter::All),
        );
        assert_eq!(by_parameters[0].name, "llama-70b");
        assert_eq!(by_parameters[2].name, "llama-3b");
    }

    #[test]
    fn refine_models_sorts_by_downloads() {
        let popular = ModelEntry {
            downloads: Some(10_000),
            ..model("popular", None, None, None, None)
        };
        let niche = ModelEntry {
            downloads: Some(10),
            ..model("niche", None, None, None, None)
        };
        let unknown = model("unknown", None, None, None, None);
        let models = vec![unknown, niche, popular];

        let descending = refine_models(
            models.clone(),
            &browse(ModelSort::DownloadsDesc, None, ModelSizeFilter::All),
        );
        assert_eq!(
            descending
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec!["popular", "niche", "unknown"]
        );

        let ascending = refine_models(
            models,
            &browse(ModelSort::DownloadsAsc, None, ModelSizeFilter::All),
        );
        assert_eq!(
            ascending
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec!["niche", "popular", "unknown"]
        );
    }

    #[test]
    fn refine_models_sorts_unknown_sizes_last() {
        let huge = model("huge", Some(9_000), None, None, None);
        let small = model("small", Some(10), None, None, None);
        let unknown = model("unknown", None, None, None, None);
        let models = vec![unknown, small, huge];

        let descending = refine_models(
            models.clone(),
            &browse(ModelSort::SizeDesc, None, ModelSizeFilter::All),
        );
        assert_eq!(
            descending
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec!["huge", "small", "unknown"]
        );

        let ascending = refine_models(
            models,
            &browse(ModelSort::SizeAsc, None, ModelSizeFilter::All),
        );
        assert_eq!(
            ascending
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec!["small", "huge", "unknown"]
        );
    }

    #[test]
    fn refine_models_sorts_by_name_and_update_time() {
        let models = vec![
            model("b", None, None, None, Some("2024-02-01")),
            model("a", None, None, None, Some("2024-03-01")),
        ];

        let descending = refine_models(
            models.clone(),
            &browse(ModelSort::NameDesc, None, ModelSizeFilter::All),
        );
        assert_eq!(descending[0].name, "b");

        let updated = refine_models(
            models,
            &browse(ModelSort::UpdatedAsc, None, ModelSizeFilter::All),
        );
        assert_eq!(updated[0].name, "b");
    }

    #[test]
    fn paginate_models_reports_the_next_offset() {
        let models: Vec<ModelEntry> = (0..5)
            .map(|index| model(&format!("m{index}"), None, None, None, None))
            .collect();

        let page = paginate_models(models, 2, 2);
        assert_eq!(page.models.len(), 2);
        assert_eq!(page.models[0].name, "m2");
        assert_eq!(page.next_cursor, Some("offset:4".to_string()));
    }

    #[test]
    fn count_matching_models_counts_without_sorting() {
        let models = vec![
            model("llama-3b", None, Some("llama"), Some("3B"), None),
            model("llama-70b", None, Some("llama"), Some("70B"), None),
            model("mistral", None, Some("mistral"), Some("7B"), None),
        ];

        assert_eq!(
            count_matching_models(
                &models,
                &browse(ModelSort::Relevance, Some("llama"), ModelSizeFilter::Small)
            ),
            1
        );
        assert_eq!(
            count_matching_models(
                &models,
                &browse(ModelSort::Relevance, None, ModelSizeFilter::All)
            ),
            3
        );
    }
}
