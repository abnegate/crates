use crate::catalog::medium_filter::ModelMediumFilter;
use crate::catalog::size_filter::ModelSizeFilter;
use crate::catalog::sort::ModelSort;

/// Options passed to a [`ModelProvider`](crate::catalog::ModelProvider) search.
#[derive(Debug, Clone, Copy)]
pub struct BrowseQuery<'a> {
    pub query: Option<&'a str>,
    pub cursor: Option<&'a str>,
    pub limit: usize,
    pub sort: ModelSort,
    pub family: Option<&'a str>,
    pub size: ModelSizeFilter,
    pub medium: ModelMediumFilter,
}
