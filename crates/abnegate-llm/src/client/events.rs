use abnegate_secret::redact;

use crate::error::LlmError;
use crate::wire::ChatStreamChunk;

const DATA_FIELD: &[u8] = b"data:";
const DONE: &[u8] = b"[DONE]";
/// The longest single event line accepted. A completion chunk is a few
/// hundred bytes; a line that runs to megabytes without a newline is a
/// broken or hostile endpoint, not a chunk worth buffering.
const MAXIMUM_LINE_BYTES: usize = 16 * 1024 * 1024;

/// Turns a server-sent event byte stream into completion chunks.
///
/// Only `data:` lines carry anything, with or without the single space the
/// format allows after the colon. `data: [DONE]` ends the stream, and so
/// does a payload that is a top-level `error` object, which is reported as
/// [`LlmError::Stream`] rather than read as an empty chunk.
pub(crate) struct EventDecoder {
    buffer: Vec<u8>,
    limit: usize,
    finished: bool,
}

impl Default for EventDecoder {
    fn default() -> Self {
        Self::with_limit(MAXIMUM_LINE_BYTES)
    }
}

impl EventDecoder {
    pub(crate) fn with_limit(limit: usize) -> Self {
        Self {
            buffer: Vec::new(),
            limit,
            finished: false,
        }
    }

    /// Whether the stream has ended, cleanly or not. Nothing after this
    /// point is decoded.
    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }

    /// Decode every complete line `bytes` finishes.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<Result<ChatStreamChunk, LlmError>> {
        let mut decoded = Vec::new();
        if self.finished {
            return decoded;
        }
        self.buffer.extend_from_slice(bytes);

        let mut consumed = 0;
        while let Some(offset) = self.buffer[consumed..]
            .iter()
            .position(|byte| *byte == b'\n')
        {
            let end = consumed + offset;
            let line = self.buffer[consumed..end].to_vec();
            consumed = end + 1;
            self.decode(&line, &mut decoded);
            if self.finished {
                self.buffer.clear();
                return decoded;
            }
        }
        self.buffer.drain(..consumed);

        if self.buffer.len() > self.limit {
            self.finished = true;
            self.buffer.clear();
            decoded.push(Err(LlmError::Stream(format!(
                "an event line ran past {} bytes without ending",
                self.limit
            ))));
        }
        decoded
    }

    /// Decode a final line the stream ended without terminating.
    pub(crate) fn finish(&mut self) -> Vec<Result<ChatStreamChunk, LlmError>> {
        let mut decoded = Vec::new();
        if !self.finished && !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            self.decode(&line, &mut decoded);
        }
        self.finished = true;
        decoded
    }

    fn decode(&mut self, line: &[u8], decoded: &mut Vec<Result<ChatStreamChunk, LlmError>>) {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(data) = line.strip_prefix(DATA_FIELD) else {
            return;
        };
        let data = data.strip_prefix(b" ").unwrap_or(data);
        if data == DONE {
            self.finished = true;
            return;
        }

        let value = match serde_json::from_slice::<serde_json::Value>(data) {
            Ok(value) => value,
            Err(error) => {
                decoded.push(Err(LlmError::Json(error)));
                return;
            }
        };
        if let Some(error) = value.get("error") {
            self.finished = true;
            let message = error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .map_or_else(|| error.to_string(), str::to_string);
            decoded.push(Err(LlmError::Stream(redact(&message).into_owned())));
            return;
        }
        decoded.push(serde_json::from_value(value).map_err(LlmError::Json));
    }
}

#[cfg(test)]
mod tests {
    use super::EventDecoder;
    use crate::error::LlmError;

    const CHUNK: &str =
        r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":null}]}"#;

    fn content(result: &Result<crate::wire::ChatStreamChunk, LlmError>) -> Option<&str> {
        result
            .as_ref()
            .ok()?
            .choices
            .first()?
            .delta
            .content
            .as_deref()
    }

    #[test]
    fn a_data_line_with_or_without_the_space_is_a_chunk() {
        let mut decoder = EventDecoder::default();

        let decoded = decoder.push(format!("data: {CHUNK}\n\ndata:{CHUNK}\n\n").as_bytes());

        assert_eq!(decoded.len(), 2);
        assert!(decoded.iter().all(|result| content(result) == Some("hi")));
    }

    #[test]
    fn a_chunk_split_across_reads_is_reassembled() {
        let mut decoder = EventDecoder::default();
        let line = format!("data: {CHUNK}\r\n");
        let (head, tail) = line.as_bytes().split_at(17);

        assert!(decoder.push(head).is_empty());
        let decoded = decoder.push(tail);

        assert_eq!(decoded.len(), 1);
        assert_eq!(content(&decoded[0]), Some("hi"));
    }

    #[test]
    fn done_ends_the_stream_and_nothing_after_it_is_read() {
        let mut decoder = EventDecoder::default();

        let decoded = decoder.push(format!("data: [DONE]\n\ndata: {CHUNK}\n").as_bytes());

        assert!(decoded.is_empty());
        assert!(decoder.is_finished());
        assert!(
            decoder
                .push(format!("data: {CHUNK}\n").as_bytes())
                .is_empty()
        );
    }

    #[test]
    fn an_error_payload_is_a_failure_not_an_empty_chunk() {
        let mut decoder = EventDecoder::default();

        let decoded = decoder.push(
            concat!(
                "data: {\"error\":{\"message\":\"overloaded, key sk-ant-",
                "api03-AAAAAAAAAAAAAAAAAAAAAAAA\"}}\n"
            )
            .as_bytes(),
        );

        assert_eq!(decoded.len(), 1);
        let Err(LlmError::Stream(message)) = &decoded[0] else {
            panic!("expected a stream failure, got {:?}", decoded[0]);
        };
        assert!(message.contains("overloaded"), "{message}");
        assert!(
            !message.contains(concat!("sk-ant-", "api03-AAAA")),
            "{message}"
        );
        assert!(decoder.is_finished());
    }

    #[test]
    fn a_final_line_without_a_newline_is_still_read() {
        let mut decoder = EventDecoder::default();

        assert!(decoder.push(format!("data: {CHUNK}").as_bytes()).is_empty());
        let decoded = decoder.finish();

        assert_eq!(decoded.len(), 1);
        assert_eq!(content(&decoded[0]), Some("hi"));
    }

    #[test]
    fn a_line_that_never_ends_is_refused_once_it_passes_the_limit() {
        let mut decoder = EventDecoder::with_limit(64);

        let decoded = decoder.push(&[b'x'; 65]);

        assert_eq!(decoded.len(), 1);
        assert!(matches!(&decoded[0], Err(LlmError::Stream(message)) if message.contains("64")));
        assert!(decoder.is_finished());
    }

    #[test]
    fn comments_and_other_fields_are_skipped() {
        let mut decoder = EventDecoder::default();

        let decoded = decoder
            .push(format!(": keep-alive\nevent: message\nid: 7\ndata: {CHUNK}\n").as_bytes());

        assert_eq!(decoded.len(), 1);
    }

    #[test]
    fn a_malformed_chunk_is_reported_and_the_stream_goes_on() {
        let mut decoder = EventDecoder::default();

        let decoded = decoder.push(format!("data: {{not json\ndata: {CHUNK}\n").as_bytes());

        assert!(matches!(decoded[0], Err(LlmError::Json(_))));
        assert_eq!(content(&decoded[1]), Some("hi"));
        assert!(!decoder.is_finished());
    }
}
