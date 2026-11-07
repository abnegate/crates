//! Stripping a query down to what full-text search can use.

const MAXIMUM_QUERY_LENGTH: usize = 500;

/// The characters of `query` that PostgreSQL `websearch_to_tsquery` can
/// usefully see for code.
///
/// The query is cut at the first character boundary at or after byte 500, then
/// everything but letters, digits, whitespace and `- " _ . / :` is dropped.
pub fn sanitize_search_query(query: &str) -> String {
    let truncated = if query.len() > MAXIMUM_QUERY_LENGTH {
        query
            .char_indices()
            .take_while(|(index, _)| *index < MAXIMUM_QUERY_LENGTH)
            .last()
            .map(|(index, character)| &query[..index + character.len_utf8()])
            .unwrap_or("")
    } else {
        query
    };

    truncated
        .chars()
        .filter(|character| {
            character.is_alphanumeric()
                || character.is_whitespace()
                || matches!(*character, '-' | '"' | '_' | '.' | '/' | ':')
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_underscores() {
        assert_eq!(
            sanitize_search_query("should_skip_blob"),
            "should_skip_blob"
        );
    }
}
