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
        let concealed = redact(&conceal(message, self.key.expose())).into_owned();
        match self.trust {
            Trust::Operator => concealed,
            Trust::Tenant => capped(&concealed),
        }
    }
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

    use futures::stream;

    use super::bounded;

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
}
