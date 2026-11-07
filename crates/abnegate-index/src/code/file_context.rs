//! The imports and module declaration at the top of a file.

#[derive(Debug, Clone, Default)]
pub(super) struct FileContext {
    pub(super) imports: String,
    pub(super) module_declaration: Option<String>,
    pub(super) file_docstring: Option<String>,
    pub(super) import_start: usize,
    pub(super) import_end: usize,
}
