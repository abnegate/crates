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
///
/// Every field is optional on the wire; start from the default and set the
/// ones a browse needs.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[non_exhaustive]
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
    /// Browse the catalogue named `source`: `ollama`, `huggingface`,
    /// `gpt4all` or `openrouter`.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Search for models matching `search`.
    pub fn with_search(mut self, search: impl Into<String>) -> Self {
        self.search = Some(search.into());
        self
    }

    /// Continue from the page `cursor` names.
    pub fn with_cursor(mut self, cursor: impl Into<String>) -> Self {
        self.cursor = Some(cursor.into());
        self
    }

    /// Ask for pages of `limit` models, held to `1..=MAXIMUM_PAGE_SIZE`.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set [`Self::sort`].
    pub fn with_sort(mut self, sort: ModelSort) -> Self {
        self.sort = Some(sort);
        self
    }

    /// Keep only models of `family`, such as `llama`; `all` keeps every one.
    pub fn with_family(mut self, family: impl Into<String>) -> Self {
        self.family = Some(family.into());
        self
    }

    /// Set [`Self::size`].
    pub fn with_size(mut self, size: ModelSizeFilter) -> Self {
        self.size = Some(size);
        self
    }

    /// Set [`Self::medium`].
    pub fn with_medium(mut self, medium: ModelMediumFilter) -> Self {
        self.medium = Some(medium);
        self
    }

    /// The page size asked for, held to `1..=MAXIMUM_PAGE_SIZE`.
    pub fn limit(&self) -> usize {
        self.limit
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAXIMUM_PAGE_SIZE)
    }

    /// This request as the query a [`ModelProvider`](crate::catalog::ModelProvider)
    /// takes, with a blank or `all` family read as no family.
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
        let request = BrowseRequest::default()
            .with_source("ollama")
            .with_search("qwen")
            .with_limit(20)
            .with_sort(ModelSort::NameAscending)
            .with_family("all");
        let browse = request.to_browse_query();
        assert_eq!(browse.query, Some("qwen"));
        assert_eq!(browse.family, None);
        assert_eq!(browse.size, ModelSizeFilter::All);
        assert_eq!(browse.medium, ModelMediumFilter::All);
    }

    #[test]
    fn browse_query_passes_medium() {
        let request = BrowseRequest::default()
            .with_source("huggingface")
            .with_cursor("offset:20")
            .with_family("llama")
            .with_size(ModelSizeFilter::Small)
            .with_medium(ModelMediumFilter::Image);
        let browse = request.to_browse_query();
        assert_eq!(browse.cursor, Some("offset:20"));
        assert_eq!(browse.family, Some("llama"));
        assert_eq!(browse.size, ModelSizeFilter::Small);
        assert_eq!(browse.medium, ModelMediumFilter::Image);
    }

    #[test]
    fn limit_defaults_and_clamps() {
        assert_eq!(BrowseRequest::default().limit(), DEFAULT_PAGE_SIZE);
        assert_eq!(BrowseRequest::default().with_limit(0).limit(), 1);
        assert_eq!(
            BrowseRequest::default().with_limit(5_000).limit(),
            MAXIMUM_PAGE_SIZE
        );
    }

    #[test]
    fn search_accepts_the_q_alias() {
        let request: BrowseRequest = serde_json::from_str(r#"{"q":"qwen"}"#).unwrap();
        assert_eq!(request.search.as_deref(), Some("qwen"));
    }
}
