use crate::catalog::medium_filter::ModelMediumFilter;
use crate::catalog::page::MAX_PAGE_SIZE;
use crate::catalog::size_filter::ModelSizeFilter;
use crate::catalog::sort::ModelSort;

/// Options passed to a [`ModelProvider`](crate::catalog::ModelProvider) search.
#[derive(Debug, Clone, Copy)]
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

impl BrowseQuery<'_> {
    /// `limit` held to `1..=MAX_PAGE_SIZE`: a page of none would name its own
    /// cursor as the next one, and a caller following it would never finish.
    pub fn page_size(&self) -> usize {
        self.limit.clamp(1, MAX_PAGE_SIZE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_limit(limit: usize) -> BrowseQuery<'static> {
        BrowseQuery {
            query: None,
            cursor: None,
            limit,
            sort: ModelSort::default(),
            family: None,
            size: ModelSizeFilter::default(),
            medium: ModelMediumFilter::default(),
        }
    }

    #[test]
    fn a_page_size_is_held_between_one_and_the_largest_page() {
        assert_eq!(with_limit(0).page_size(), 1);
        assert_eq!(with_limit(20).page_size(), 20);
        assert_eq!(with_limit(5_000).page_size(), MAX_PAGE_SIZE);
    }
}
