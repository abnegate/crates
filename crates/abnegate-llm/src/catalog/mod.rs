//! Browsing the model catalogues Ollama, HuggingFace, GPT4All and OpenRouter
//! publish, behind one [`ModelProvider`] trait.
//!
//! ```no_run
//! use abnegate_llm::catalog::BrowseRequest;
//! use abnegate_llm::catalog::browse;
//!
//! # async fn example() -> Result<(), abnegate_llm::catalog::CatalogError> {
//! let request = BrowseRequest {
//!     search: Some("qwen".into()),
//!     ..Default::default()
//! };
//! let page = browse("huggingface")?
//!     .search(request.to_browse_query())
//!     .await?;
//!
//! for model in page.models {
//!     println!("{}", model.name);
//! }
//! # Ok(())
//! # }
//! ```

mod browse;
mod capability;
mod details;
mod entry;
mod error;
mod gpt4all;
mod http;
mod huggingface;
#[cfg(test)]
mod listening;
mod medium_filter;
mod ollama;
mod openrouter;
mod page;
mod parse;
mod provider;
mod query;
mod refine;
mod request;
mod size;
mod size_filter;
mod sort;
mod text;

pub use browse::browse;
pub use browse::browse_with_proxy;
pub use capability::ModelCapability;
pub use details::ModelDetails;
pub use entry::ModelEntry;
pub use error::CatalogError;
pub use gpt4all::DEFAULT_GPT4ALL_MODELS_URL;
pub use gpt4all::Gpt4AllProvider;
pub use huggingface::DEFAULT_HUGGINGFACE_MODELS_URL;
pub use huggingface::HuggingFaceProvider;
pub use huggingface::huggingface_hub_origin;
pub use huggingface::huggingface_repo_downloads;
pub use huggingface::huggingface_repo_id;
pub use medium_filter::ModelMediumFilter;
pub use ollama::DEFAULT_OLLAMA_REGISTRY_URL;
pub use ollama::DEFAULT_OLLAMA_SEARCH_URL;
pub use ollama::OllamaProvider;
pub use openrouter::DEFAULT_OPENROUTER_MODELS_URL;
pub use openrouter::OpenRouterProvider;
pub use page::DEFAULT_PAGE_SIZE;
pub use page::MAX_PAGE_SIZE;
pub use page::ModelPage;
pub use parse::extract_model_family;
pub use parse::extract_parameter_size;
pub use parse::extract_quantization;
pub use provider::ModelProvider;
pub use query::BrowseQuery;
pub use request::BrowseRequest;
pub use size::ModelSize;
pub use size_filter::ModelSizeFilter;
pub use sort::ModelSort;
