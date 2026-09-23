use crate::catalog::error::CatalogError;
use crate::catalog::gpt4all::DEFAULT_GPT4ALL_MODELS_URL;
use crate::catalog::gpt4all::Gpt4AllProvider;
use crate::catalog::huggingface::DEFAULT_HUGGINGFACE_MODELS_URL;
use crate::catalog::huggingface::HuggingFaceProvider;
use crate::catalog::ollama::OllamaProvider;
use crate::catalog::openrouter::OpenRouterProvider;
use crate::catalog::page::ModelPage;
use crate::catalog::query::BrowseQuery;
use async_trait::async_trait;

/// A browsable remote model catalogue.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn search(&self, options: BrowseQuery<'_>) -> Result<ModelPage, CatalogError>;
}

/// Build a provider by catalogue name.
pub fn provider(name: &str) -> Result<Box<dyn ModelProvider>, CatalogError> {
    provider_with_proxy(name, None)
}

/// Build a provider by catalogue name, routing catalogue requests through the
/// given HTTP proxy.
pub fn provider_with_proxy(
    name: &str,
    proxy_url: Option<&str>,
) -> Result<Box<dyn ModelProvider>, CatalogError> {
    match name {
        "ollama" => Ok(Box::new(OllamaProvider::with_proxy(proxy_url)?)),
        "huggingface" => Ok(Box::new(HuggingFaceProvider::with_proxy(
            DEFAULT_HUGGINGFACE_MODELS_URL,
            proxy_url,
        )?)),
        "gpt4all" => Ok(Box::new(Gpt4AllProvider::with_proxy(
            DEFAULT_GPT4ALL_MODELS_URL,
            proxy_url,
        )?)),
        "openrouter" => Ok(Box::new(OpenRouterProvider::with_proxy(
            crate::catalog::openrouter::DEFAULT_OPENROUTER_MODELS_URL,
            proxy_url,
        )?)),
        _ => Err(CatalogError::Unavailable(format!(
            "Unknown provider: {name}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_resolves_every_catalogue() {
        assert_eq!(provider("ollama").unwrap().name(), "ollama");
        assert_eq!(provider("huggingface").unwrap().name(), "huggingface");
        assert_eq!(provider("gpt4all").unwrap().name(), "gpt4all");
        assert_eq!(provider("openrouter").unwrap().name(), "openrouter");
        assert!(provider("unknown").is_err());
    }

    #[test]
    fn provider_is_object_safe() {
        let provider: Box<dyn ModelProvider> = Box::new(OllamaProvider::new());
        assert_eq!(provider.name(), "ollama");
    }

    #[test]
    fn provider_with_proxy_reports_a_malformed_proxy() {
        assert!(provider_with_proxy("ollama", Some("not a url")).is_err());
    }
}
