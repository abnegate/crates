use crate::catalog::error::CatalogError;
use crate::catalog::page::ModelPage;
use crate::catalog::query::BrowseQuery;
use async_trait::async_trait;

/// A browsable remote model catalogue.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn search(&self, options: BrowseQuery<'_>) -> Result<ModelPage, CatalogError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::ollama::OllamaProvider;

    #[test]
    fn provider_is_object_safe() {
        let provider: Box<dyn ModelProvider> = Box::new(OllamaProvider::new().unwrap());
        assert_eq!(provider.name(), "ollama");
    }
}
