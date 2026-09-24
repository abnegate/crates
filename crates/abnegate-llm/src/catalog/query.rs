use crate::catalog::medium_filter::ModelMediumFilter;
use crate::catalog::page::DEFAULT_PAGE_SIZE;
use crate::catalog::page::MAXIMUM_PAGE_SIZE;
use crate::catalog::size_filter::ModelSizeFilter;
use crate::catalog::sort::ModelSort;

/// Options passed to a [`ModelProvider`](crate::catalog::ModelProvider) search.
///
/// The default is the first page of every model, in relevance order.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct BrowseQuery<'a> {
    pub query: Option<&'a str>,
    pub cursor: Option<&'a str>,
    /// The page size asked for, which a provider reads through
    /// [`Self::page_size`].
    pub limit: usize,
    pub sort: ModelSort,
    pub family: Option<&'a str>,
    pub size: ModelSizeFilter,
    pub medium: ModelMediumFilter,
}

impl Default for BrowseQuery<'_> {
    fn default() -> Self {
        Self {
            query: None,
            cursor: None,
            limit: DEFAULT_PAGE_SIZE,
            sort: ModelSort::default(),
            family: None,
            size: ModelSizeFilter::default(),
            medium: ModelMediumFilter::default(),
        }
    }
}

impl<'a> BrowseQuery<'a> {
    /// Search for models matching `query`.
    pub fn with_query(mut self, query: &'a str) -> Self {
        self.query = Some(query);
        self
    }

    /// Continue from the page `cursor` names.
    pub fn with_cursor(mut self, cursor: &'a str) -> Self {
        self.cursor = Some(cursor);
        self
    }

    /// Set [`Self::limit`].
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Set [`Self::sort`].
    pub fn with_sort(mut self, sort: ModelSort) -> Self {
        self.sort = sort;
        self
    }

    /// Keep only models of `family`, such as `llama`.
    pub fn with_family(mut self, family: &'a str) -> Self {
        self.family = Some(family);
        self
    }

    /// Set [`Self::size`].
    pub fn with_size(mut self, size: ModelSizeFilter) -> Self {
        self.size = size;
        self
    }

    /// Set [`Self::medium`].
    pub fn with_medium(mut self, medium: ModelMediumFilter) -> Self {
        self.medium = medium;
        self
    }

    /// `limit` held to `1..=MAXIMUM_PAGE_SIZE`: a page of none would name its
    /// own cursor as the next one, and a caller following it would never
    /// finish.
    pub fn page_size(&self) -> usize {
        self.limit.clamp(1, MAXIMUM_PAGE_SIZE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_limit(limit: usize) -> BrowseQuery<'static> {
        BrowseQuery::default().with_limit(limit)
    }

    #[test]
    fn a_page_size_is_held_between_one_and_the_largest_page() {
        assert_eq!(with_limit(0).page_size(), 1);
        assert_eq!(with_limit(20).page_size(), 20);
        assert_eq!(with_limit(5_000).page_size(), MAXIMUM_PAGE_SIZE);
    }

    #[test]
    fn the_default_asks_for_the_first_page_of_everything() {
        let query = BrowseQuery::default();

        assert_eq!(query.query, None);
        assert_eq!(query.cursor, None);
        assert_eq!(query.limit, DEFAULT_PAGE_SIZE);
        assert_eq!(query.sort, ModelSort::Relevance);
        assert_eq!(query.family, None);
        assert_eq!(query.size, ModelSizeFilter::All);
        assert_eq!(query.medium, ModelMediumFilter::All);
    }

    #[test]
    fn every_option_can_be_set() {
        let query = BrowseQuery::default()
            .with_query("qwen")
            .with_cursor("offset:20")
            .with_limit(10)
            .with_sort(ModelSort::NameAscending)
            .with_family("qwen")
            .with_size(ModelSizeFilter::Small)
            .with_medium(ModelMediumFilter::Text);

        assert_eq!(query.query, Some("qwen"));
        assert_eq!(query.cursor, Some("offset:20"));
        assert_eq!(query.limit, 10);
        assert_eq!(query.sort, ModelSort::NameAscending);
        assert_eq!(query.family, Some("qwen"));
        assert_eq!(query.size, ModelSizeFilter::Small);
        assert_eq!(query.medium, ModelMediumFilter::Text);
    }
}
