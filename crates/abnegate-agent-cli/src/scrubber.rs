//! Keeping the secrets a run was given out of everything it writes down.

use std::borrow::Cow;
use std::sync::Arc;

use abnegate_secret::REDACTED;
use abnegate_secret::SecretValue;
use abnegate_secret::redact;

/// A secret shorter than this is too likely to occur inside other words for
/// every occurrence to be replaced, so only a whole-word occurrence is.
const MINIMUM: usize = 8;

const QUOTE: char = '"';
const BACKSLASH: u8 = b'\\';
const ESCAPED_LETTERS: &[u8] = b"ntrbf";
const HEX_DIGITS: usize = 4;
const UNICODE_ESCAPE: usize = 6;
const UNRESERVED: [char; 4] = ['-', '.', '_', '~'];
const ENCODED_SPACE: &str = "%20";
const FORM_SPACE: &str = "+";

/// Redacts credential-shaped text, and every secret value this run was
/// handed, from text on its way to a log, an error or a caller.
///
/// Pattern redaction alone misses a secret that does not look like one, such
/// as a password, and an agent is as likely to echo that as anything else.
/// Each secret is looked for as written, JSON-escaped, since the agent's
/// stream and the journal are both JSON, and percent-encoded, since agents
/// echo URLs; a short secret is replaced only where it stands as a word of
/// its own, and never skipped.
#[derive(Debug, Clone, Default)]
pub(crate) struct Scrubber {
    anywhere: Arc<[SecretValue]>,
    words: Arc<[SecretValue]>,
}

impl Scrubber {
    /// A scrubber for every value in `secrets`, which should be everything
    /// the run's child was handed that is not public.
    pub(crate) fn new(secrets: impl IntoIterator<Item = SecretValue>) -> Self {
        let mut anywhere: Vec<SecretValue> = Vec::new();
        let mut words: Vec<SecretValue> = Vec::new();
        for secret in secrets {
            let value = secret.expose();
            if value.trim().is_empty() {
                continue;
            }
            let forms = if value.len() < MINIMUM {
                &mut words
            } else {
                &mut anywhere
            };
            for form in forms_of(value) {
                let form = SecretValue::new(form);
                if !forms.contains(&form) {
                    forms.push(form);
                }
            }
        }
        for forms in [&mut anywhere, &mut words] {
            forms.sort_by_key(|form| std::cmp::Reverse(form.expose().len()));
        }
        Self {
            anywhere: anywhere.into(),
            words: words.into(),
        }
    }

    /// `text` with every configured secret replaced, longest first, and
    /// then anything else credential-shaped. The configured secrets go
    /// first so that pattern redaction cannot take part of one and leave the
    /// rest of it unrecognisable.
    pub(crate) fn scrub<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut text = Cow::Borrowed(text);
        for secret in self.anywhere.iter() {
            if text.contains(secret.expose()) {
                text = Cow::Owned(text.replace(secret.expose(), REDACTED));
            }
        }
        for secret in self.words.iter() {
            if let Some(replaced) = replace_words(&text, secret.expose()) {
                text = Cow::Owned(replaced);
            }
        }
        let redacted = match redact(&text) {
            Cow::Owned(redacted) => Some(redacted),
            Cow::Borrowed(_) => None,
        };
        redacted.map_or(text, Cow::Owned)
    }
}

/// `value` as written, JSON-escaped, and percent-encoded with either case of
/// hex digit and with a space as `+`.
fn forms_of(value: &str) -> Vec<String> {
    let escaped = serde_json::to_string(value)
        .ok()
        .and_then(|quoted| {
            quoted
                .strip_prefix(QUOTE)
                .and_then(|quoted| quoted.strip_suffix(QUOTE))
                .map(str::to_string)
        })
        .unwrap_or_else(|| value.to_string());
    let encoded = percent_encoded(value);
    let lowercase = encoded
        .split('%')
        .enumerate()
        .map(|(index, part)| match index {
            0 => part.to_string(),
            _ => {
                let split = part.len().min(2);
                format!("%{}{}", part[..split].to_ascii_lowercase(), &part[split..])
            }
        })
        .collect::<String>();
    let form = encoded.replace(ENCODED_SPACE, FORM_SPACE);
    vec![value.to_string(), escaped, encoded, lowercase, form]
}

fn percent_encoded(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        let character = char::from(byte);
        if character.is_ascii_alphanumeric() || UNRESERVED.contains(&character) {
            encoded.push(character);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// `text` with every occurrence of `word` that no letter, digit or
/// underscore adjoins replaced, or `None` when there is none. The letter or
/// digits ending a JSON escape, such as the `n` of `\n`, do not count as
/// adjoining, since the text they stand for does not.
fn replace_words(text: &str, word: &str) -> Option<String> {
    let mut replaced = String::with_capacity(text.len());
    let mut copied = 0;
    let mut from = 0;
    while let Some(offset) = text[from..].find(word) {
        let start = from + offset;
        let end = start + word.len();
        if adjoined_before(&text[..start]) || text[end..].chars().next().is_some_and(wordlike) {
            from = start + text[start..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        replaced.push_str(&text[copied..start]);
        replaced.push_str(REDACTED);
        copied = end;
        from = end;
    }
    if copied == 0 {
        return None;
    }
    replaced.push_str(&text[copied..]);
    Some(replaced)
}

fn wordlike(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn adjoined_before(before: &str) -> bool {
    before.chars().next_back().is_some_and(wordlike) && !escaped(before)
}

/// Whether `before` ends in a JSON escape standing for a character that is
/// not part of a word: `\n`, `\t`, `\r`, `\b`, `\f` or `\uXXXX`, opened by
/// a backslash that is not itself escaped.
fn escaped(before: &str) -> bool {
    let bytes = before.as_bytes();
    let length = bytes.len();
    let opening = if length >= UNICODE_ESCAPE
        && bytes[length - UNICODE_ESCAPE + 1] == b'u'
        && bytes[length - HEX_DIGITS..]
            .iter()
            .all(u8::is_ascii_hexdigit)
    {
        length - UNICODE_ESCAPE
    } else if length >= 2 && ESCAPED_LETTERS.contains(&bytes[length - 1]) {
        length - 2
    } else {
        return false;
    };
    let backslashes = bytes[..=opening]
        .iter()
        .rev()
        .take_while(|byte| **byte == BACKSLASH)
        .count();
    backslashes % 2 == 1
}

#[cfg(test)]
mod tests {
    use abnegate_secret::SecretValue;

    use super::Scrubber;

    fn scrubber(secrets: &[&str]) -> Scrubber {
        Scrubber::new(secrets.iter().map(|secret| SecretValue::new(*secret)))
    }

    #[test]
    fn a_secret_that_does_not_look_like_one_is_still_removed() {
        let scrubber = scrubber(&["correct horse battery", "hunter2hunter2", "plainpassword1"]);

        let scrubbed = scrubber.scrub(
            "login failed for correct horse battery, then hunter2hunter2, then plainpassword1",
        );
        assert_eq!(
            scrubbed,
            "login failed for [REDACTED], then [REDACTED], then [REDACTED]"
        );
    }

    #[test]
    fn credential_shaped_text_is_redacted_even_when_it_was_never_configured() {
        let scrubbed = Scrubber::default().scrub(concat!(
            "fatal: bad key sk-ant-",
            "api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        ));
        assert!(
            !scrubbed.contains(concat!("sk-ant-", "api03-AAAA")),
            "{scrubbed}"
        );
    }

    #[test]
    fn a_json_escaped_secret_is_removed_from_a_json_line() {
        let secret = "pa\"ss\\word-123";
        let line =
            serde_json::json!({ "type": "assistant", "text": format!("key {secret}") }).to_string();
        assert!(
            !line.contains(secret),
            "the line must hold the escaped form"
        );

        let scrubbed = scrubber(&[secret]).scrub(&line);

        assert!(!scrubbed.contains("word-123"), "{scrubbed}");
        assert!(scrubbed.contains("[REDACTED]"), "{scrubbed}");
        assert!(serde_json::from_str::<serde_json::Value>(&scrubbed).is_ok());
    }

    #[test]
    fn a_percent_encoded_secret_is_removed_from_a_url() {
        let scrubber = scrubber(&["p@ss word/1"]);

        for url in [
            "https://example.com/?password=p%40ss%20word%2F1&next=1",
            "https://example.com/?password=p%40ss%20word%2f1&next=1",
            "https://example.com/?password=p%40ss+word%2F1&next=1",
        ] {
            assert_eq!(
                scrubber.scrub(url),
                "https://example.com/?password=[REDACTED]&next=1",
                "{url}"
            );
        }
    }

    #[test]
    fn a_short_secret_is_removed_wherever_it_stands_as_a_word() {
        let scrubber = scrubber(&["abc", "1"]);

        assert_eq!(
            scrubber.scrub("token abc rejected; retry with abc."),
            "token [REDACTED] rejected; retry with [REDACTED]."
        );
        assert_eq!(
            scrubber.scrub("exited with 1 error"),
            "exited with [REDACTED] error"
        );
        assert_eq!(
            scrubber.scrub("abcdef and 10 errors_abc"),
            "abcdef and 10 errors_abc"
        );
    }

    #[test]
    fn a_short_secret_after_a_json_escape_is_still_a_word_of_its_own() {
        let scrubber = scrubber(&["ab12cd"]);

        for line in [
            serde_json::json!({ "text": "Your code is:\nab12cd" }).to_string(),
            serde_json::json!({ "text": "code\tab12cd\r\n" }).to_string(),
            r#"{"text":"code\u00a0ab12cd"}"#.to_string(),
        ] {
            let scrubbed = scrubber.scrub(&line);
            assert!(!scrubbed.contains("ab12cd"), "{line} became {scrubbed}");
        }
        assert_eq!(
            scrubber.scrub(r#"{"text":"C:\\nab12cd"}"#),
            r#"{"text":"C:\\nab12cd"}"#,
            "an escaped backslash followed by n is a real letter"
        );
    }

    #[test]
    fn a_short_secret_next_to_a_rejected_occurrence_is_still_found() {
        assert_eq!(scrubber(&["a-a"]).scrub("xa-a-a"), "xa-[REDACTED]");
    }

    #[test]
    fn pattern_redaction_never_splits_a_configured_secret() {
        let scrubbed =
            scrubber(&["Sup3r-sk-Pr0duction!x"]).scrub("login Sup3r-sk-Pr0duction!x failed");
        assert_eq!(scrubbed, "login [REDACTED] failed");
    }

    #[test]
    fn a_short_secret_is_removed_in_its_escaped_form_too() {
        let scrubber = scrubber(&["a\"b"]);
        let line = serde_json::json!({ "text": "key a\"b rejected" }).to_string();

        let scrubbed = scrubber.scrub(&line);

        assert!(!scrubbed.contains("a\\\"b"), "{scrubbed}");
        assert!(scrubbed.contains("[REDACTED]"), "{scrubbed}");
    }

    #[test]
    fn an_empty_or_blank_secret_redacts_nothing() {
        assert_eq!(scrubber(&["", "  "]).scrub("a  b"), "a  b");
    }

    #[test]
    fn a_longer_secret_is_replaced_before_one_it_contains() {
        let scrubbed = scrubber(&["hunter2hunter2", "hunter2hunter2-extended"])
            .scrub("key hunter2hunter2-extended");
        assert_eq!(scrubbed, "key [REDACTED]");
    }

    #[test]
    fn debug_never_prints_a_secret() {
        assert!(!format!("{:?}", scrubber(&["hunter2hunter2", "abc"])).contains("hunter2"));
    }
}
