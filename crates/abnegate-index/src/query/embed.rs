//! Query text for instruction-aware embedding models.

/// The text to embed for `query` under `model`.
///
/// Qwen3-Embedding is instruction-aware, so its queries carry a retrieval
/// instruction. Every other model, and every document, embeds raw text.
pub fn embed_query_text(model: &str, query: &str) -> String {
    if model_family(model).contains("qwen") {
        format!(
            "Instruct: Given a search query, retrieve relevant code passages that answer it.\nQuery: {query}"
        )
    } else {
        query.to_string()
    }
}

fn model_family(model: &str) -> String {
    model
        .split(':')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qwen_queries_get_an_instruction() {
        let text = embed_query_text("qwen3-embedding:0.6b", "should_skip_blob");
        assert!(text.contains("Instruct:"));
        assert!(text.contains("should_skip_blob"));
        assert_eq!(embed_query_text("nomic-embed-text", "hello"), "hello");
    }
}
