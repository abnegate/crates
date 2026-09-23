use once_cell::sync::Lazy;
use regex::Regex;
use std::cmp::Ordering;

static PARAM_SIZE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[^A-Za-z0-9])(\d+(?:\.\d+)?)\s*[bB]\b").expect("param size regex")
});
static PARAM_SIZE_CHIP_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^\d+(?:\.\d+)?[bB]$").expect("param size chip regex"));
static BILLION_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(\d+(?:\.\d+)?)\s*billion").expect("billion regex"));
/// Longer GGUF tags first so `Q4_K_M` wins over a shorter neighbour.
static QUANT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(?:^|[^A-Za-z0-9])(IQ1_S|IQ1_M|IQ2_XXS|IQ2_XS|IQ2_S|IQ2_M|IQ3_XXS|IQ3_XS|IQ3_S|IQ3_M|IQ4_XS|IQ4_NL|Q2_K_S|Q2_K|Q3_K_L|Q3_K_M|Q3_K_S|Q4_K_M|Q4_K_S|Q4_1|Q4_0|Q5_K_M|Q5_K_S|Q5_1|Q5_0|Q6_K|Q8_0|BF16|F16|F32)(?:[^A-Za-z0-9]|$)",
    )
    .expect("quant regex")
});

/// Parameter size read out of a model name, such as `7B` or `1B · 3B`.
pub fn extract_param_size(name: &str) -> Option<String> {
    format_param_sizes(extract_all_param_sizes(name))
        .or_else(|| BILLION_RE.captures(name).map(|cap| format!("{}B", &cap[1])))
}

/// Quantization tag read out of a GGUF filename, such as `Q5_K_M`.
pub fn extract_quantization(filename: &str) -> Option<String> {
    QUANT_RE.captures(filename).map(|cap| cap[1].to_uppercase())
}

/// Model family read out of a model name.
///
/// Longer, more specific names are tried first so `codellama` is not
/// classified as `llama`.
pub fn extract_model_family(name: &str) -> Option<String> {
    let lowered = name.to_lowercase();
    let families = [
        ("codellama", "codellama"),
        ("codegemma", "codegemma"),
        ("starcoder", "starcoder"),
        ("deepseek", "deepseek"),
        ("nemotron", "nemotron"),
        ("snowflake", "snowflake"),
        ("minimax", "minimax"),
        ("mixtral", "mixtral"),
        ("mistral", "mistral"),
        ("command", "command"),
        ("granite", "granite"),
        ("smollm", "smollm"),
        ("ornith", "ornith"),
        ("llama", "llama"),
        ("qwen", "qwen"),
        ("gemma", "gemma"),
        ("vicuna", "vicuna"),
        ("falcon", "falcon"),
        ("nomic", "nomic"),
        ("mxbai", "mxbai"),
        ("muse", "muse"),
        ("kimi", "kimi"),
        ("phi", "phi"),
        ("glm", "glm"),
        ("yi", "yi"),
    ];

    for (pattern, family) in families {
        if lowered.contains(pattern) {
            return Some(family.to_string());
        }
    }
    None
}

pub(crate) fn extract_all_param_sizes(text: &str) -> Vec<String> {
    let mut sizes = Vec::new();
    for capture in PARAM_SIZE_RE.captures_iter(text) {
        let formatted = format!("{}B", &capture[1]);
        if !sizes.iter().any(|existing| existing == &formatted) {
            sizes.push(formatted);
        }
    }
    sort_param_sizes(&mut sizes);
    sizes
}

pub(crate) fn collect_param_size_labels(text: &str, chips: Vec<String>) -> Vec<String> {
    let mut sizes = extract_all_param_sizes(text);
    for chip in chips {
        let formatted = chip.to_uppercase();
        if !sizes.iter().any(|existing| existing == &formatted) {
            sizes.push(formatted);
        }
    }
    sort_param_sizes(&mut sizes);
    sizes.dedup();
    sizes
}

pub(crate) fn format_param_sizes(mut sizes: Vec<String>) -> Option<String> {
    sort_param_sizes(&mut sizes);
    sizes.dedup();
    match sizes.len() {
        0 => None,
        1 => Some(sizes.remove(0)),
        2 | 3 => Some(sizes.join(" · ")),
        _ => Some(format!("{}–{}", sizes[0], sizes[sizes.len() - 1])),
    }
}

fn sort_param_sizes(sizes: &mut [String]) {
    sizes.sort_by(|left, right| {
        param_size_value(left)
            .partial_cmp(&param_size_value(right))
            .unwrap_or(Ordering::Equal)
    });
}

fn param_size_value(label: &str) -> f64 {
    label
        .trim()
        .trim_end_matches(['B', 'b'])
        .parse()
        .unwrap_or(0.0)
}

pub(crate) fn is_param_size_chip(text: &str) -> bool {
    PARAM_SIZE_CHIP_RE.is_match(text.trim())
}

pub(crate) fn normalize_parameter_label(raw: &str) -> String {
    if let Some(capture) = BILLION_RE.captures(raw) {
        return format!("{}B", &capture[1]);
    }
    extract_param_size(raw).unwrap_or_else(|| raw.to_string())
}

pub(crate) fn parse_param_billions(raw: &str) -> Option<f64> {
    let lowered = raw.to_lowercase();

    if let Some(prefix) = lowered.split("billion").next()
        && lowered.contains("billion")
    {
        return prefix.split_whitespace().rev().find_map(|token| {
            token
                .trim_matches(|character: char| !character.is_ascii_digit() && character != '.')
                .parse()
                .ok()
        });
    }

    if let Some(prefix) = lowered.split("million").next()
        && lowered.contains("million")
    {
        return prefix
            .split_whitespace()
            .rev()
            .find_map(|token| {
                token
                    .trim_matches(|character: char| !character.is_ascii_digit() && character != '.')
                    .parse::<f64>()
                    .ok()
            })
            .map(|millions| millions / 1000.0);
    }

    let mut number = String::new();
    let mut unit = None::<char>;

    for character in lowered.chars() {
        if character.is_ascii_digit() || (character == '.' && !number.contains('.')) {
            number.push(character);
            continue;
        }

        if !number.is_empty() {
            if character == 'b' || character == 'm' {
                unit = Some(character);
            }
            break;
        }
    }

    if number.is_empty() {
        return None;
    }

    let value: f64 = number.parse().ok()?;
    match unit {
        Some('m') => Some(value / 1000.0),
        Some('b') | None => Some(value),
        _ => None,
    }
}

pub(crate) fn download_param_billions(label: &str) -> Option<f64> {
    label
        .split_once('·')
        .map(|(prefix, _)| prefix.trim())
        .and_then(parse_param_billions)
}

fn quantization_token(label: &str) -> Option<&str> {
    QUANT_RE
        .captures(label)
        .and_then(|capture| capture.get(1).map(|token| token.as_str()))
}

pub(crate) fn quantization_bit_width(label: &str) -> Option<u8> {
    let token = quantization_token(label)?.to_uppercase();
    if matches!(token.as_str(), "F16" | "BF16") {
        return Some(16);
    }
    if token == "F32" {
        return Some(32);
    }
    let digits: String = token
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

pub(crate) fn quantization_preference(label: &str) -> u8 {
    let token = quantization_token(label).unwrap_or(label).to_uppercase();
    if token.contains("K_M") {
        0
    } else if token.ends_with("_0") {
        1
    } else if token.contains("K_S") {
        2
    } else if token.ends_with("_1") {
        3
    } else if token.contains("K_L") {
        4
    } else {
        5
    }
}

pub(crate) fn parse_compact_count(raw: &str) -> Option<u64> {
    let trimmed = raw.trim().replace(',', "");
    if trimmed.is_empty() {
        return None;
    }
    let (number, multiplier) = match trimmed.chars().last()? {
        'K' | 'k' => (&trimmed[..trimmed.len() - 1], 1_000.0),
        'M' | 'm' => (&trimmed[..trimmed.len() - 1], 1_000_000.0),
        _ => (trimmed.as_str(), 1.0),
    };
    let parsed: f64 = number.trim().parse().ok()?;
    Some((parsed * multiplier).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_param_size_reads_names() {
        assert_eq!(extract_param_size("llama-7b"), Some("7B".to_string()));
        assert_eq!(
            extract_param_size("mistral-13B-v2"),
            Some("13B".to_string())
        );
        assert_eq!(extract_param_size("qwen-72b-chat"), Some("72B".to_string()));
        assert_eq!(extract_param_size("phi-3b"), Some("3B".to_string()));
        assert_eq!(extract_param_size("model-without-size"), None);
        assert_eq!(
            extract_param_size("llama-70b-chat"),
            Some("70B".to_string())
        );
        assert_eq!(
            extract_param_size("granite 3B 8B 30B"),
            Some("3B · 8B · 30B".to_string())
        );
        assert_eq!(extract_param_size("8 billion"), Some("8B".to_string()));
        assert_eq!(
            extract_param_size("Qwen3-Coder-30B-A3B-Instruct-GGUF"),
            Some("30B".to_string())
        );
    }

    #[test]
    fn extract_param_size_reads_repository_names() {
        assert_eq!(
            extract_param_size("Qwen/Qwen2.5-Coder-7B-Instruct-GGUF"),
            Some("7B".to_string())
        );
        assert_eq!(
            extract_param_size("meta-llama/Llama-3-13B-GGUF"),
            Some("13B".to_string())
        );
        assert_eq!(
            extract_param_size("model-1.5b-instruct"),
            Some("1.5B".to_string())
        );
        assert_eq!(extract_param_size("no-params-here"), None);
    }

    #[test]
    fn extract_quantization_reads_filenames() {
        assert_eq!(
            extract_quantization("model-Q4_0.gguf"),
            Some("Q4_0".to_string())
        );
        assert_eq!(
            extract_quantization("llama-7b-Q5_K_M.gguf"),
            Some("Q5_K_M".to_string())
        );
        assert_eq!(
            extract_quantization("model-Q8_0.gguf"),
            Some("Q8_0".to_string())
        );
        assert_eq!(
            extract_quantization("model-IQ4_XS.gguf"),
            Some("IQ4_XS".to_string())
        );
        assert_eq!(extract_quantization("model.gguf"), None);
        assert_eq!(
            extract_quantization("model.BF16.gguf"),
            Some("BF16".to_string())
        );
        assert_eq!(
            extract_quantization("model-Q4_K_M-00001-of-00002.gguf"),
            Some("Q4_K_M".to_string())
        );
    }

    #[test]
    fn extract_quantization_reads_lowercase_filenames() {
        assert_eq!(
            extract_quantization("model-q4_k_m.gguf"),
            Some("Q4_K_M".to_string())
        );
        assert_eq!(
            extract_quantization("model-q8_0.gguf"),
            Some("Q8_0".to_string())
        );
        assert_eq!(
            extract_quantization("model-q5_k_s.gguf"),
            Some("Q5_K_S".to_string())
        );
    }

    #[test]
    fn extract_model_family_reads_names() {
        assert_eq!(extract_model_family("llama3.2"), Some("llama".to_string()));
        assert_eq!(
            extract_model_family("mistral-7b"),
            Some("mistral".to_string())
        );
        assert_eq!(extract_model_family("qwen2.5"), Some("qwen".to_string()));
        assert_eq!(extract_model_family("phi3"), Some("phi".to_string()));
        assert_eq!(
            extract_model_family("deepseek-r1"),
            Some("deepseek".to_string())
        );
        assert_eq!(extract_model_family("unknown-model"), None);
        assert_eq!(
            extract_model_family("codellama"),
            Some("codellama".to_string())
        );
        assert_eq!(extract_model_family("glm-5.3"), Some("glm".to_string()));
    }

    #[test]
    fn extract_model_family_reads_repository_names() {
        assert_eq!(
            extract_model_family("Qwen/Qwen2.5-Coder-7B"),
            Some("qwen".to_string())
        );
        assert_eq!(
            extract_model_family("meta-llama/Llama-3-8B"),
            Some("llama".to_string())
        );
        assert_eq!(
            extract_model_family("mistralai/Mistral-7B"),
            Some("mistral".to_string())
        );
    }

    #[test]
    fn parse_param_billions_reads_labels() {
        assert_eq!(parse_param_billions("7B"), Some(7.0));
        assert_eq!(parse_param_billions("3.8B"), Some(3.8));
        assert_eq!(parse_param_billions("7 billion"), Some(7.0));
        assert_eq!(parse_param_billions("137M"), Some(0.137));
        assert_eq!(parse_param_billions("335 million"), Some(0.335));
        assert_eq!(parse_param_billions("unknown"), None);
    }

    #[test]
    fn parse_compact_count_reads_suffixes() {
        assert_eq!(parse_compact_count("28.7K"), Some(28_700));
        assert_eq!(parse_compact_count("1.4M"), Some(1_400_000));
        assert_eq!(parse_compact_count("1234"), Some(1234));
        assert_eq!(parse_compact_count(""), None);
    }

    #[test]
    fn quantization_ordering_prefers_k_m() {
        assert_eq!(quantization_bit_width("Q4_K_M"), Some(4));
        assert_eq!(quantization_bit_width("BF16"), Some(16));
        assert_eq!(quantization_bit_width("F32"), Some(32));
        assert_eq!(quantization_bit_width("no-quant"), None);
        assert!(quantization_preference("Q4_K_M") < quantization_preference("Q4_0"));
        assert!(quantization_preference("Q4_0") < quantization_preference("Q4_K_S"));
        assert_eq!(download_param_billions("8B · Q4_K_M"), Some(8.0));
        assert_eq!(download_param_billions("Q4_K_M"), None);
    }

    #[test]
    fn normalize_parameter_label_prefers_billions() {
        assert_eq!(normalize_parameter_label("8 billion"), "8B");
        assert_eq!(normalize_parameter_label("7B"), "7B");
        assert_eq!(normalize_parameter_label("unknown"), "unknown");
    }
}
