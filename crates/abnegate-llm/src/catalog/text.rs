use scraper::Html;

pub(crate) fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn html_to_plain_text(html: &str) -> String {
    let fragment = Html::parse_fragment(html);
    collapse_whitespace(&fragment.root_element().text().collect::<Vec<_>>().join(" "))
}

pub(crate) fn nonempty_vec(values: Vec<String>) -> Option<Vec<String>> {
    (!values.is_empty()).then_some(values)
}

pub(crate) fn humanize_label(raw: &str) -> String {
    raw.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            match characters.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), characters.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn format_context_tokens(tokens: u64) -> String {
    const BINARY_WINDOWS: &[u64] = &[
        4_096, 8_192, 16_384, 32_768, 65_536, 131_072, 262_144, 524_288, 1_048_576, 2_097_152,
        4_194_304, 8_388_608,
    ];
    if BINARY_WINDOWS.contains(&tokens) {
        if tokens >= 1_048_576 {
            return format!("{}M", tokens / 1_048_576);
        }
        return format!("{}K", tokens / 1024);
    }
    if tokens >= 1_000_000 && tokens.is_multiple_of(1_000_000) {
        return format!("{}M", tokens / 1_000_000);
    }
    if tokens >= 1_000_000 {
        return format!("{:.1}M", tokens as f64 / 1_000_000.0);
    }
    if tokens >= 1000 {
        return format!("{}K", tokens / 1000);
    }
    tokens.to_string()
}

pub(crate) fn use_cases_from_pipeline(pipeline: Option<&str>, tags: &[String]) -> Vec<String> {
    let mut cases = Vec::new();
    if let Some(tag) = pipeline {
        let mapped = match tag {
            "text-generation" | "text2text-generation" | "conversational" => Some("Chat"),
            "feature-extraction" | "sentence-similarity" => Some("Embeddings"),
            "text-to-image" | "image-to-image" | "image-text-to-image" => Some("Image generation"),
            "automatic-speech-recognition" | "text-to-speech" | "audio-to-audio" => Some("Audio"),
            "image-text-to-text" | "image-to-text" => Some("Vision"),
            _ => None,
        };
        if let Some(label) = mapped {
            cases.push(label.to_string());
        }
    }
    let extras = infer_use_cases(
        &std::iter::once(pipeline.unwrap_or(""))
            .chain(tags.iter().map(String::as_str))
            .collect::<Vec<_>>(),
    );
    for label in extras {
        if !cases.iter().any(|existing| existing == &label) {
            cases.push(label);
        }
    }
    cases.truncate(5);
    cases
}

pub(crate) fn infer_use_cases(parts: &[&str]) -> Vec<String> {
    let blob = parts.join(" ").to_lowercase();
    let mut cases = Vec::new();
    let rules = [
        ("vision", "Vision"),
        ("image", "Vision"),
        ("multimodal", "Multimodal"),
        ("function call", "Tool use"),
        ("tool_choice", "Tool use"),
        ("tools", "Tool use"),
        ("tool use", "Tool use"),
        ("thinking", "Reasoning"),
        ("reasoning", "Reasoning"),
        ("include_reasoning", "Reasoning"),
        ("embedding", "Embeddings"),
        ("audio", "Audio"),
        ("speech", "Audio"),
        ("coding", "Coding"),
        ("code", "Coding"),
        ("agent", "Agents"),
        ("instruct", "Chat"),
        ("conversational", "Chat"),
        ("chat", "Chat"),
    ];
    for (needle, label) in rules {
        if blob.contains(needle) && !cases.iter().any(|existing| existing == label) {
            cases.push(label.to_string());
        }
    }
    cases.truncate(5);
    cases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_to_plain_text_strips_markup() {
        assert_eq!(
            html_to_plain_text("<ul><li>Use for complex reasoning tasks</li></ul>"),
            "Use for complex reasoning tasks"
        );
    }

    #[test]
    fn collapse_whitespace_joins_runs() {
        assert_eq!(collapse_whitespace("  a \n b   c "), "a b c");
    }

    #[test]
    fn humanize_label_titlecases_segments() {
        assert_eq!(humanize_label("text-generation"), "Text Generation");
        assert_eq!(humanize_label("qwen3moe"), "Qwen3moe");
    }

    #[test]
    fn format_context_tokens_uses_binary_windows() {
        assert_eq!(format_context_tokens(262_144), "256K");
        assert_eq!(format_context_tokens(1_048_576), "1M");
        assert_eq!(format_context_tokens(128_000), "128K");
        assert_eq!(format_context_tokens(512), "512");
    }

    #[test]
    fn nonempty_vec_drops_empty() {
        assert_eq!(nonempty_vec(Vec::new()), None);
        assert_eq!(nonempty_vec(vec!["a".into()]), Some(vec!["a".to_string()]));
    }

    #[test]
    fn infer_use_cases_reads_keywords() {
        assert_eq!(infer_use_cases(&["a chat model"]), vec!["Chat".to_string()]);
        assert!(infer_use_cases(&["tools"]).contains(&"Tool use".to_string()));
        assert!(infer_use_cases(&[""]).is_empty());
    }

    #[test]
    fn use_cases_from_pipeline_leads_with_the_task() {
        let cases = use_cases_from_pipeline(Some("text-generation"), &["conversational".into()]);
        assert_eq!(cases.first().map(String::as_str), Some("Chat"));
    }
}
