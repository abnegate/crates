//! Prompt templates: `{{key}}` substitution, `{{#if key}}` sections and
//! `{{#each key}}` loops over a key/value [`TemplateContext`].
//!
//! Rendering is a single pass over the template, so a value is never read as
//! template text: a title that happens to contain `{{secret}}` renders as
//! written rather than expanding it.

mod context;
mod renderer;

pub use context::TemplateContext;
pub use renderer::TemplateRenderer;
