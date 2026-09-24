use std::sync::Arc;

use crate::provider::completion_provider::CompletionProvider;

/// A provider and its share of a weighted split.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Weighted {
    pub provider: Arc<dyn CompletionProvider>,
    pub weight: f64,
}

impl Weighted {
    pub fn new(provider: Arc<dyn CompletionProvider>, weight: f64) -> Self {
        Self { provider, weight }
    }

    /// An arm that is never chosen by weight but can still back another up.
    pub fn spare(provider: Arc<dyn CompletionProvider>) -> Self {
        Self::new(provider, 0.0)
    }
}
