use thiserror::Error;

/// Why [`TemplateRenderer::render_strict`](super::TemplateRenderer::render_strict)
/// refused a template.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum TemplateError {
    #[error("the template uses {0:?}, which the context does not hold")]
    Missing(String),
}
