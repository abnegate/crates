use thiserror::Error;

/// Why [`TemplateRenderer::render_strict`](super::TemplateRenderer::render_strict)
/// refused a template.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum TemplateError {
    /// The template names this key, and neither the context nor the
    /// enclosing `{{#each}}` item holds it.
    #[error("the template uses {0:?}, which the context does not hold")]
    Missing(String),
}
