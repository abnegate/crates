//! Language label from a relative path.

use std::path::Path;

/// Lowercased file extension of `relative`, or empty when there is none.
pub fn language_of(relative: &str) -> String {
    Path::new(relative)
        .extension()
        .and_then(|item| item.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_is_the_lowercased_extension() {
        assert_eq!(language_of("lib/api.rb"), "rb");
        assert_eq!(language_of("Source/JSC.CPP"), "cpp");
        assert_eq!(language_of("README"), "");
    }
}
