use crate::catalog::medium_filter::ModelMediumFilter;
use crate::catalog::page::DEFAULT_PAGE_SIZE;
use crate::catalog::page::MAXIMUM_PAGE_SIZE;
use crate::catalog::query::BrowseQuery;
use crate::catalog::size_filter::ModelSizeFilter;
use crate::catalog::sort::ModelSort;
use serde::Deserialize;
use serde::Serialize;

/// A browse as it arrives over the wire, before it is borrowed as a
/// [`BrowseQuery`].
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct BrowseRequest {
    /// Catalogue to browse: `ollama`, `huggingface`, `gpt4all` or `openrouter`.
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default, alias = "q")]
    pub search: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub sort: Option<ModelSort>,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub size: Option<ModelSizeFilter>,
    #[serde(default)]
    pub medium: Option<ModelMediumFilter>,
}

impl BrowseRequest {
    /// The page size asked for, held to `1..=MAXIMUM_PAGE_SIZE`.
    pub fn limit(&self) -> usize {
        self.limit
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAXIMUM_PAGE_SIZE)
    }

    pub fn to_browse_query(&self) -> BrowseQuery<'_> {
        let family = self
            .family
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("all"));

        BrowseQuery {
            query: self.search.as_deref(),
            cursor: self.cursor.as_deref(),
            limit: self.limit(),
            sort: self.sort.unwrap_or_default(),
            family,
            size: self.size.unwrap_or_default(),
            medium: self.medium.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browse_query_defaults_blank_filters() {
        let request = BrowseRequest {
            source: Some("ollama".into()),
            search: Some("qwen".into()),
            cursor: None,
            limit: Some(20),
            sort: Some(ModelSort::NameAscending),
            family: Some("all".into()),
            size: None,
            medium: None,
        };
        let browse = request.to_browse_query();
        assert_eq!(browse.query, Some("qwen"));
        assert_eq!(browse.family, None);
        assert_eq!(browse.size, ModelSizeFilter::All);
        assert_eq!(browse.medium, ModelMediumFilter::All);
    }

    #[test]
    fn browse_query_passes_medium() {
        let request = BrowseRequest {
            source: Some("huggingface".into()),
            search: None,
            cursor: None,
            limit: None,
            sort: None,
            family: Some("llama".into()),
            size: Some(ModelSizeFilter::Small),
            medium: Some(ModelMediumFilter::Image),
        };
        let browse = request.to_browse_query();
        assert_eq!(browse.family, Some("llama"));
        assert_eq!(browse.size, ModelSizeFilter::Small);
        assert_eq!(browse.medium, ModelMediumFilter::Image);
    }

    #[test]
    fn limit_defaults_and_clamps() {
        assert_eq!(BrowseRequest::default().limit(), DEFAULT_PAGE_SIZE);
        assert_eq!(
            BrowseRequest {
                limit: Some(0),
                ..Default::default()
            }
            .limit(),
            1
        );
        assert_eq!(
            BrowseRequest {
                limit: Some(5_000),
                ..Default::default()
            }
            .limit(),
            MAXIMUM_PAGE_SIZE
        );
    }

    #[test]
    fn search_accepts_the_q_alias() {
        let request: BrowseRequest = serde_json::from_str(r#"{"q":"qwen"}"#).unwrap();
        assert_eq!(request.search.as_deref(), Some("qwen"));
    }
}
