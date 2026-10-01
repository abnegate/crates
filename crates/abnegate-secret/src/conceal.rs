use crate::redact::REDACTED;

const MINIMUM_ECHO: usize = 3;
const MASK: char = '*';

/// `text` without `key`, whether it appears whole or masked the way providers
/// echo a rejected key: `sk-ab****wxyz`, `****wxyz` or `sk-ab****`.
///
/// A masked word is concealed when the run before or after its mask is at
/// least three characters and matches the start or end of `key`. A blank `key`
/// leaves `text` untouched.
///
/// ```
/// use abnegate_secret::conceal;
///
/// let key = concat!("sk-proj-", "AbCdEfGh1234567890wxyz");
///
/// assert_eq!(
///     conceal("Incorrect API key provided: sk-proj-****wxyz.", key),
///     "Incorrect API key provided: [REDACTED]"
/// );
/// ```
pub fn conceal(text: &str, key: &str) -> String {
    let key = key.trim();
    if key.is_empty() {
        return text.to_string();
    }
    let whole = text.replace(key, REDACTED);
    let mut output = String::with_capacity(whole.len());
    let mut word = String::new();
    for character in whole.chars() {
        if separates(character) {
            output.push_str(unechoed(&word, key));
            word.clear();
            output.push(character);
        } else {
            word.push(character);
        }
    }
    output.push_str(unechoed(&word, key));
    output
}

fn separates(character: char) -> bool {
    character.is_whitespace()
        || matches!(
            character,
            '"' | '\''
                | '`'
                | ','
                | ';'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '<'
                | '>'
                | '='
                | ':'
                | '\\'
        )
}

fn unechoed<'a>(word: &'a str, key: &str) -> &'a str {
    let (Some(first), Some(last)) = (word.find(MASK), word.rfind(MASK)) else {
        return word;
    };
    let prefix = word[..first].trim_start_matches(|character: char| !character.is_alphanumeric());
    let suffix = word[last + MASK.len_utf8()..]
        .trim_end_matches(|character: char| !character.is_alphanumeric());
    let echoes = |part: &str| part.len() >= MINIMUM_ECHO;
    if (echoes(prefix) && key.starts_with(prefix)) || (echoes(suffix) && key.ends_with(suffix)) {
        REDACTED
    } else {
        word
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "sk-proj-AbCdEfGh1234567890wxyz";

    #[test]
    fn every_echo_of_the_key_is_concealed() {
        for reported in [
            format!("API error (401): bad key {KEY}"),
            "Incorrect API key provided: sk-proj-****************wxyz. You can find your API key at https://platform.openai.com/account/api-keys.".to_string(),
            r#"{"error":{"message":"invalid x-api-key: ****wxyz"}}"#.to_string(),
            r#"{\"error\":{\"message\":\"Incorrect API key provided: sk-proj-****wxyz\"}}"#.to_string(),
            r#"{\"error\":\"bad key ****wxyz\\\"}"#.to_string(),
            "rejected api_key=sk-proj-Ab****".to_string(),
            "rejected key:sk-proj-Ab****".to_string(),
            "rejected key:sk-proj-Ab****!".to_string(),
            "invalid: sk-proj-****wxyz\\n".to_string(),
        ] {
            let concealed = conceal(&reported, KEY);

            assert!(!concealed.contains(KEY), "{concealed}");
            assert!(!concealed.contains("wxyz"), "{concealed}");
            assert!(!concealed.contains("sk-proj-Ab"), "{concealed}");
            assert!(concealed.contains(REDACTED), "{concealed}");
        }
    }

    #[test]
    fn text_without_the_key_is_left_alone() {
        let failure = "API error (404): The model `gpt-9` does not exist ** or key=abc** you do not have access.";

        assert_eq!(conceal(failure, "plainkey-9999"), failure);
        assert_eq!(conceal(failure, "  "), failure);
    }
}
