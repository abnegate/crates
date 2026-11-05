use abnegate_secret::REDACTED;
use abnegate_secret::SecretValue;
use abnegate_secret::conceal;
use abnegate_secret::redact;
use futures::Stream;
use futures::StreamExt;
use reqwest::StatusCode;
use serde_json::Value;

use crate::client::trust::Trust;
use crate::error::Error;

pub(crate) const REPORTED_LIMIT: usize = 500;
pub(crate) const ELLIPSIS: char = '…';
pub(crate) const UNEXPLAINED: &str = "the endpoint reported an error";
const MINIMUM_CONCEALED_KEY_LENGTH: usize = 8;
const TOKEN_SEPARATORS: [char; 16] = [
    '"', '\'', '`', ',', ';', '(', ')', '[', ']', '{', '}', '<', '>', '=', ':', '\\',
];
const TERMINAL_PUNCTUATION: [char; 2] = ['.', '!'];
const MESSAGE_POINTERS: [&str; 3] = ["/error/message", "/error", "/message"];
const TENANT_BODY_LIMIT: usize = 64 * 1024;

/// How what an endpoint says about a failure reaches the caller: without the
/// key the client sent it or anything [`redact`] recognises as a secret, and,
/// for a [`Trust::Tenant`] endpoint, as nothing but the endpoint's own error
/// message, capped at [`REPORTED_LIMIT`] characters.
#[derive(Debug, Clone)]
pub(crate) struct Reporter {
    trust: Trust,
    key: SecretValue,
}

impl Reporter {
    pub(crate) fn new(trust: Trust, key: SecretValue) -> Self {
        Self { trust, key }
    }

    /// The endpoint's refusal of a request. An operator's endpoint is
    /// reported with its whole body; a tenant's by its status and its own
    /// error message in OpenAI's error envelope.
    ///
    /// A tenant's body is read no further than [`TENANT_BODY_LIMIT`] bytes,
    /// so one that is huge or never ends costs no more than that.
    pub(crate) async fn rejected(&self, response: reqwest::Response) -> Error {
        let status = response.status();
        let message = match self.trust {
            Trust::Operator => self.shaped(&response.text().await.unwrap_or_default()),
            Trust::Tenant => self.enveloped(
                status,
                &bounded(response.bytes_stream(), TENANT_BODY_LIMIT).await,
            ),
        };
        Error::Api {
            status: status.as_u16(),
            message,
        }
    }

    /// The message for the `error` a stream sent in place of a chunk. A
    /// tenant's endpoint is reported only by a string it sent as the error's
    /// message, or as the error itself, and otherwise by [`UNEXPLAINED`].
    pub(crate) fn streamed(&self, error: &Value) -> String {
        let message = error.get("message").and_then(Value::as_str);
        match self.trust {
            Trust::Operator => {
                self.shaped(&message.map_or_else(|| error.to_string(), str::to_string))
            }
            Trust::Tenant => self.shaped(message.or(error.as_str()).unwrap_or(UNEXPLAINED)),
        }
    }

    fn enveloped(&self, status: StatusCode, body: &str) -> String {
        let reported = reported(body)
            .unwrap_or_else(|| status.canonical_reason().unwrap_or_default().to_string());
        serde_json::json!({ "error": { "message": self.shaped(&reported) } }).to_string()
    }

    fn shaped(&self, message: &str) -> String {
        let concealed = redact(&conceal_key(message, self.key.expose())).into_owned();
        match self.trust {
            Trust::Operator => concealed,
            Trust::Tenant => capped(&concealed),
        }
    }
}

/// `text` with every echo of `key` concealed.
///
/// A trimmed `key` of at least [`MINIMUM_CONCEALED_KEY_LENGTH`] characters is
/// concealed by [`conceal`], wherever it appears, whole or masked. A shorter
/// one, such as `sk-1234` or `hunter2`, is concealed only where it stands as
/// a whole token, so that a placeholder such as `len` or `token` cannot
/// garble "the input length" or "too many tokens". A blank `key` leaves
/// `text` untouched.
pub(crate) fn conceal_key(text: &str, key: &str) -> String {
    let key = key.trim();
    if key.is_empty() {
        return text.to_string();
    }
    if key.chars().count() < MINIMUM_CONCEALED_KEY_LENGTH {
        return conceal_tokens(text, key);
    }
    conceal(text, key)
}

/// `text` with every token that is `key` replaced by [`REDACTED`], and every
/// separator around it kept as it was.
fn conceal_tokens(text: &str, key: &str) -> String {
    let mut concealed = String::with_capacity(text.len());
    for piece in text.split_inclusive(separates) {
        let token = piece.strip_suffix(separates).unwrap_or(piece);
        match echoed(token, key) {
            Some(punctuation) => {
                concealed.push_str(REDACTED);
                concealed.push_str(punctuation);
            }
            None => concealed.push_str(token),
        }
        concealed.push_str(&piece[token.len()..]);
    }
    concealed
}

/// The [`TERMINAL_PUNCTUATION`] that follows `key` in `token`, empty when
/// `token` is `key` itself, or `None` when `token` is not an echo of `key`.
fn echoed<'a>(token: &'a str, key: &str) -> Option<&'a str> {
    if token == key {
        return Some("");
    }
    token
        .strip_suffix(TERMINAL_PUNCTUATION)
        .filter(|stripped| *stripped == key)
        .map(|stripped| &token[stripped.len()..])
}

fn separates(character: char) -> bool {
    character.is_whitespace() || TOKEN_SEPARATORS.contains(&character)
}

/// At most `limit` bytes of `body`, read chunk by chunk and no further, as
/// text.
async fn bounded<Chunk, Failure>(
    body: impl Stream<Item = Result<Chunk, Failure>>,
    limit: usize,
) -> String
where
    Chunk: AsRef<[u8]>,
{
    let mut body = std::pin::pin!(body);
    let mut read = Vec::new();
    while read.len() < limit {
        match body.next().await {
            Some(Ok(chunk)) => {
                let chunk = chunk.as_ref();
                read.extend_from_slice(&chunk[..chunk.len().min(limit - read.len())]);
            }
            Some(Err(_)) | None => break,
        }
    }
    String::from_utf8_lossy(&read).into_owned()
}

/// The error message a refusal's body carries, as OpenAI, Anthropic, LiteLLM
/// and Ollama each shape it.
fn reported(body: &str) -> Option<String> {
    let body = serde_json::from_str::<Value>(body).ok()?;
    MESSAGE_POINTERS
        .iter()
        .find_map(|pointer| {
            body.pointer(pointer)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|message| !message.is_empty())
        })
        .map(str::to_string)
}

fn capped(message: &str) -> String {
    match message.char_indices().nth(REPORTED_LIMIT) {
        Some((end, _)) => format!("{}{ELLIPSIS}", &message[..end]),
        None => message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use abnegate_secret::REDACTED;
    use abnegate_secret::SecretValue;
    use futures::stream;
    use serde_json::json;

    use super::*;

    const CHUNK: usize = 16 * 1024;

    #[tokio::test]
    async fn a_body_is_read_no_further_than_the_limit() {
        let read = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&read);
        let endless = stream::repeat_with(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok::<_, std::io::Error>(vec![b'x'; CHUNK])
        });

        let body = bounded(endless, 4 * CHUNK + 1).await;

        assert_eq!(body.len(), 4 * CHUNK + 1);
        assert_eq!(read.load(Ordering::SeqCst), 5, "chunks read past the limit");
    }

    const CONTEXT_LENGTH: &str = "the input length exceeds the context length";
    const KEY: &str = concat!("sk-", "proxy-master-0123456789");
    const SHORT_KEY: &str = concat!("sk-", "1234");
    const LITELLM_REJECTED_KEY: &str = "Authentication Error, Invalid proxy server token passed. Received API Key = sk-1234, Key Hash (Token) =9f86d081884c7d65";

    #[test]
    fn a_short_key_never_mangles_the_words_around_it() {
        for key in ["x", "token", "len", "  len  "] {
            let reporter = Reporter::new(Trust::Operator, SecretValue::new(key));

            let reported = reporter.streamed(&json!({ "message": CONTEXT_LENGTH }));

            assert_eq!(reported, CONTEXT_LENGTH, "{key:?} mangled the message");
        }
    }

    #[test]
    fn a_short_key_echoed_whole_is_concealed() {
        for trust in [Trust::Operator, Trust::Tenant] {
            let reporter = Reporter::new(trust, SecretValue::new(SHORT_KEY));

            let reported = reporter.streamed(&json!({ "message": LITELLM_REJECTED_KEY }));

            assert_eq!(
                reported,
                LITELLM_REJECTED_KEY.replace(SHORT_KEY, REDACTED),
                "{trust:?}"
            );
            assert!(!reported.contains(SHORT_KEY), "{trust:?}: {reported}");
        }

        let reporter = Reporter::new(Trust::Operator, SecretValue::new("hunter2"));

        let reported = reporter.streamed(&json!({ "message": "the password was hunter2." }));

        assert_eq!(reported, format!("the password was {REDACTED}."));
    }

    #[test]
    fn a_short_key_is_concealed_between_any_separators() {
        let echoed = "key=\"sk-1234\"; (sk-1234) `sk-1234`!\tsk-1234! [sk-1234]:sk-1234\\n";

        let concealed = conceal_key(echoed, &format!("  {SHORT_KEY}  "));

        assert_eq!(concealed, echoed.replace(SHORT_KEY, REDACTED));
    }

    #[test]
    fn a_short_key_inside_a_longer_token_is_left_alone() {
        let text = "sk-12345 xsk-1234 sk-1234.5 sk-1234?";

        assert_eq!(conceal_key(text, SHORT_KEY), text);
    }

    #[test]
    fn a_blank_key_leaves_the_text_untouched() {
        for key in ["", "   "] {
            assert_eq!(conceal_key(LITELLM_REJECTED_KEY, key), LITELLM_REJECTED_KEY);
        }
    }

    #[test]
    fn a_long_key_is_still_concealed() {
        let masked = format!("{}****{}", &KEY[..6], &KEY[KEY.len() - 4..]);
        for echoed in [KEY.to_string(), masked] {
            for trust in [Trust::Operator, Trust::Tenant] {
                let reporter = Reporter::new(trust, SecretValue::new(KEY));

                let reported = reporter.streamed(&json!({
                    "message": format!("{CONTEXT_LENGTH}; Received API Key = {echoed}")
                }));

                assert!(!reported.contains(&echoed), "{trust:?}: {reported}");
                assert!(reported.contains(REDACTED), "{trust:?}: {reported}");
                assert!(reported.contains(CONTEXT_LENGTH), "{trust:?}: {reported}");
            }
        }
    }

    #[test]
    fn a_key_of_the_minimum_length_is_concealed() {
        let key = "k".repeat(MINIMUM_CONCEALED_KEY_LENGTH);

        let concealed = conceal_key(&format!("bad key {key}"), &key);

        assert_eq!(concealed, format!("bad key {REDACTED}"));
    }
}
