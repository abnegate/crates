use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use async_trait::async_trait;

use crate::modality::RerankProvider;
use crate::provider::error::ProviderError;

type Scorer = dyn Fn(&str, &str) -> f32 + Send + Sync;

/// A [`RerankProvider`] whose scores a test decides, and which remembers how
/// often it was asked.
pub struct StubReranker {
    name: String,
    scoring: Result<Box<Scorer>, String>,
    calls: AtomicUsize,
}

impl StubReranker {
    /// A stub called `name` that scores each document as
    /// `scorer(query, document)` says.
    pub fn new(
        name: impl Into<String>,
        scorer: impl Fn(&str, &str) -> f32 + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            scoring: Ok(Box::new(scorer)),
            calls: AtomicUsize::new(0),
        }
    }

    /// A stub that fails every call with a [`ProviderError::Network`] saying
    /// `message`, as an unreachable endpoint would.
    pub fn failing(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            scoring: Err(message.into()),
            calls: AtomicUsize::new(0),
        }
    }

    /// The stub behind a shared handle.
    pub fn shared(self) -> Arc<dyn RerankProvider> {
        Arc::new(self)
    }

    /// How many calls to [`RerankProvider::rerank`] and
    /// [`RerankProvider::score`] the stub has been handed.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn scorer(&self) -> Result<&Scorer, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.scoring.as_deref().map_err(ProviderError::network)
    }
}

impl fmt::Debug for StubReranker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StubReranker")
            .field("name", &self.name)
            .field("failure", &self.scoring.as_ref().err())
            .field("calls", &self.calls)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl RerankProvider for StubReranker {
    fn name(&self) -> &str {
        &self.name
    }

    async fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<f32>, ProviderError> {
        let scorer = self.scorer()?;
        Ok(documents
            .iter()
            .map(|document| scorer(query, document))
            .collect())
    }

    async fn score(&self, query: &str, document: &str) -> Result<f32, ProviderError> {
        Ok(self.scorer()?(query, document))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlap(query: &str, document: &str) -> f32 {
        let matched = query
            .split_whitespace()
            .filter(|word| document.contains(word))
            .count();
        matched as f32
    }

    #[tokio::test]
    async fn a_stub_scores_with_its_closure_and_counts_its_calls() {
        let stub = StubReranker::new("stub", overlap);
        let documents = vec!["skip blob".to_string(), "login form".to_string()];

        let scores = stub.rerank("skip the blob", &documents).await.unwrap();
        let score = stub.score("login form", "login form").await.unwrap();

        assert_eq!(scores, vec![2.0, 0.0]);
        assert_eq!(score, 2.0);
        assert_eq!(stub.calls(), 2);
        assert_eq!(stub.model(), "stub");
    }

    #[tokio::test]
    async fn a_failing_stub_fails_every_call_and_still_counts_it() {
        let stub = StubReranker::failing("stub", "connection refused");

        let batch = stub.rerank("query", &["document".to_string()]).await;
        let single = stub.score("query", "document").await;

        for error in [batch.unwrap_err(), single.unwrap_err()] {
            assert!(
                matches!(&error, ProviderError::Network { detail } if detail == "connection refused"),
                "{error:?}"
            );
        }
        assert_eq!(stub.calls(), 2);
    }

    #[test]
    fn a_shared_stub_is_a_rerank_provider() {
        let shared = StubReranker::new("stub", |_, _| 0.5).shared();

        assert_eq!(shared.name(), "stub");
    }

    #[test]
    fn a_printed_stub_names_its_failure() {
        let printed = format!("{:?}", StubReranker::failing("stub", "down"));

        assert!(printed.contains("down"), "{printed}");
    }
}
