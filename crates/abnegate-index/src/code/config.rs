//! How the code chunker sizes and labels its chunks.

const MAXIMUM_SYMBOL_TOKENS: usize = 512;

const MINIMUM_SYMBOL_TOKENS: usize = 30;

const MAXIMUM_IMPORT_TOKENS: usize = 100;

/// How the code chunker sizes and labels its chunks.
///
/// The default allows 512 tokens a chunk, merges symbols under 30 tokens,
/// spends at most 100 tokens on imports, has no file path, and generates
/// skeleton and API surface chunks.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ChunkConfig {
    /// The tokens a chunk aims to stay within. A chunk can be over it. From a
    /// budget of 2 up, one over the larger of 6 characters a token and 512
    /// characters is cut into text windows; below that, nothing is cut.
    pub maximum_tokens: usize,
    /// Symbols under this many tokens are merged with their siblings.
    pub minimum_tokens: usize,
    /// Most tokens of imports a body chunk carries.
    pub maximum_import_tokens: usize,
    /// The file path written into chunk headers and metadata.
    pub file_path: Option<String>,
    /// Whether to add a skeleton chunk for each symbol.
    pub generate_skeletons: bool,
    /// Whether to add a chunk listing the file's public signatures.
    pub generate_api_surface: bool,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            maximum_tokens: MAXIMUM_SYMBOL_TOKENS,
            minimum_tokens: MINIMUM_SYMBOL_TOKENS,
            maximum_import_tokens: MAXIMUM_IMPORT_TOKENS,
            file_path: None,
            generate_skeletons: true,
            generate_api_surface: true,
        }
    }
}

impl ChunkConfig {
    /// Set the most tokens a chunk may hold before it is split.
    pub fn with_maximum_tokens(mut self, maximum_tokens: usize) -> Self {
        self.maximum_tokens = maximum_tokens;
        self
    }

    /// Set the token count under which symbols are merged with their siblings.
    pub fn with_minimum_tokens(mut self, minimum_tokens: usize) -> Self {
        self.minimum_tokens = minimum_tokens;
        self
    }

    /// Set the most tokens of imports a body chunk carries.
    pub fn with_maximum_import_tokens(mut self, maximum_import_tokens: usize) -> Self {
        self.maximum_import_tokens = maximum_import_tokens;
        self
    }

    /// Set the file path written into chunk headers and metadata.
    pub fn with_file_path(mut self, file_path: impl Into<String>) -> Self {
        self.file_path = Some(file_path.into());
        self
    }

    /// Set whether to add a skeleton chunk for each symbol.
    pub fn with_generate_skeletons(mut self, generate_skeletons: bool) -> Self {
        self.generate_skeletons = generate_skeletons;
        self
    }

    /// Set whether to add a chunk listing the file's public signatures.
    pub fn with_generate_api_surface(mut self, generate_api_surface: bool) -> Self {
        self.generate_api_surface = generate_api_surface;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_the_donor_limits() {
        let config = ChunkConfig::default();
        assert_eq!(config.maximum_tokens, 512);
        assert_eq!(config.minimum_tokens, 30);
        assert_eq!(config.maximum_import_tokens, 100);
        assert_eq!(config.file_path, None);
        assert!(config.generate_skeletons);
        assert!(config.generate_api_surface);
    }

    #[test]
    fn builders_set_each_field() {
        let config = ChunkConfig::default()
            .with_maximum_tokens(64)
            .with_minimum_tokens(8)
            .with_maximum_import_tokens(16)
            .with_file_path("src/lib.rs")
            .with_generate_skeletons(false)
            .with_generate_api_surface(false);
        assert_eq!(config.maximum_tokens, 64);
        assert_eq!(config.minimum_tokens, 8);
        assert_eq!(config.maximum_import_tokens, 16);
        assert_eq!(config.file_path.as_deref(), Some("src/lib.rs"));
        assert!(!config.generate_skeletons);
        assert!(!config.generate_api_surface);
    }
}
