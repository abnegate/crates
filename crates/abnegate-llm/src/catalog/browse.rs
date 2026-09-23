use crate::catalog::error::CatalogError;
use crate::catalog::gpt4all::DEFAULT_GPT4ALL_MODELS_URL;
use crate::catalog::gpt4all::Gpt4AllProvider;
use crate::catalog::huggingface::DEFAULT_HUGGINGFACE_MODELS_URL;
use crate::catalog::huggingface::HuggingFaceProvider;
use crate::catalog::ollama::OllamaProvider;
use crate::catalog::openrouter::DEFAULT_OPENROUTER_MODELS_URL;
use crate::catalog::openrouter::OpenRouterProvider;
use crate::catalog::provider::ModelProvider;

/// The catalogue named `name`: `ollama`, `huggingface`, `gpt4all` or
/// `openrouter`.
pub fn browse(name: &str) -> Result<Box<dyn ModelProvider>, CatalogError> {
    browse_with_proxy(name, None)
}

/// The catalogue named `name`, reached through the HTTP proxy at `proxy_url`
/// when one is given.
pub fn browse_with_proxy(
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
            DEFAULT_OPENROUTER_MODELS_URL,
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
    fn every_catalogue_is_found_by_name() {
        assert_eq!(browse("ollama").unwrap().name(), "ollama");
        assert_eq!(browse("huggingface").unwrap().name(), "huggingface");
        assert_eq!(browse("gpt4all").unwrap().name(), "gpt4all");
        assert_eq!(browse("openrouter").unwrap().name(), "openrouter");
        assert!(browse("unknown").is_err());
    }

    #[test]
    fn a_malformed_proxy_is_reported() {
        assert!(browse_with_proxy("ollama", Some("not a url")).is_err());
    }
}
