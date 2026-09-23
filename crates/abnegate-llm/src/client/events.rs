use abnegate_secret::redact;

use crate::error::LlmError;
use crate::wire::ChatStreamChunk;

const COMMENT: u8 = b':';
const DATA_FIELD: &[u8] = b"data";
const DONE: &[u8] = b"[DONE]";
/// The longest event, and so the longest single line, accepted. A
/// completion chunk is a few hundred bytes; an event that runs to megabytes
/// is a broken or hostile endpoint, not a chunk worth buffering.
const MAXIMUM_EVENT_BYTES: usize = 16 * 1024 * 1024;

/// Turns a server-sent event byte stream into completion chunks.
///
/// Lines are gathered into events as the format defines them: the values of
/// an event's `data` lines, each with or without the single space allowed
/// after the colon, are joined with newlines, and the event is dispatched at
/// the blank line that ends it, or at the end of the stream for a last event
/// left open. Comments and every other field are skipped.
///
/// `[DONE]` ends the stream, and so does a payload that is a top-level
/// `error` object, which is reported as [`LlmError::Stream`] rather than read
/// as an empty chunk. A stream that ends without a single event is a
/// failure too, not an empty answer.
pub(crate) struct EventDecoder {
    buffer: Vec<u8>,
    /// How much of `buffer` is known to hold no newline, so a line that
    /// arrives over many reads is searched once rather than once per read.
    scanned: usize,
    data: Vec<u8>,
    received: u64,
    dispatched: bool,
    limit: usize,
    finished: bool,
}

impl Default for EventDecoder {
    fn default() -> Self {
        Self::with_limit(MAXIMUM_EVENT_BYTES)
    }
}

impl EventDecoder {
    pub(crate) fn with_limit(limit: usize) -> Self {
        Self {
            buffer: Vec::new(),
            scanned: 0,
            data: Vec::new(),
            received: 0,
            dispatched: false,
            limit,
            finished: false,
        }
    }

    /// Whether the stream has ended, cleanly or not. Nothing after this
    /// point is decoded.
    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }

    /// Decode every event `bytes` completes.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<Result<ChatStreamChunk, LlmError>> {
        let mut decoded = Vec::new();
        if self.finished {
            return decoded;
        }
        self.received += bytes.len() as u64;
        self.buffer.extend_from_slice(bytes);

        let mut consumed = 0;
        let mut searched = self.scanned;
        while let Some(offset) = self.buffer[searched..]
            .iter()
            .position(|byte| *byte == b'\n')
        {
            let end = searched + offset;
            let line = self.buffer[consumed..end].to_vec();
            consumed = end + 1;
            searched = consumed;
            self.line(&line, &mut decoded);
            if self.finished {
                return decoded;
            }
        }
        self.buffer.drain(..consumed);
        self.scanned = self.buffer.len();

        if self.buffer.len() > self.limit {
            self.fail(
                format!("an event line ran past {} bytes without ending", self.limit),
                &mut decoded,
            );
        }
        decoded
    }

    /// Decode what the stream left unterminated when it ended, and fail a
    /// stream that never sent an event.
    pub(crate) fn finish(&mut self) -> Vec<Result<ChatStreamChunk, LlmError>> {
        let mut decoded = Vec::new();
        if !self.finished {
            let line = std::mem::take(&mut self.buffer);
            if !line.is_empty() {
                self.line(&line, &mut decoded);
            }
            if !self.finished {
                self.dispatch(&mut decoded);
            }
            if !self.finished && !self.dispatched {
                decoded.push(Err(LlmError::Stream(format!(
                    "the stream ended after {} bytes without a single event",
                    self.received
                ))));
            }
        }
        self.finished = true;
        decoded
    }

    fn line(&mut self, line: &[u8], decoded: &mut Vec<Result<ChatStreamChunk, LlmError>>) {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            self.dispatch(decoded);
            return;
        }
        if line.first() == Some(&COMMENT) {
            return;
        }
        let (field, value) = match line.iter().position(|byte| *byte == b':') {
            Some(colon) => (&line[..colon], &line[colon + 1..]),
            None => (line, &line[line.len()..]),
        };
        if field != DATA_FIELD {
            return;
        }

        let value = value.strip_prefix(b" ").unwrap_or(value);
        self.data.extend_from_slice(value);
        self.data.push(b'\n');
        if self.data.len() > self.limit {
            self.fail(format!("an event ran past {} bytes", self.limit), decoded);
        }
    }

    fn dispatch(&mut self, decoded: &mut Vec<Result<ChatStreamChunk, LlmError>>) {
        let mut data = std::mem::take(&mut self.data);
        if data.pop().is_none() {
            return;
        }
        self.dispatched = true;
        if data == DONE {
            self.finished = true;
            return;
        }

        let value = match serde_json::from_slice::<serde_json::Value>(&data) {
            Ok(value) => value,
            Err(error) => {
                decoded.push(Err(LlmError::Json(error)));
                return;
            }
        };
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .map_or_else(|| error.to_string(), str::to_string);
            self.fail(redact(&message).into_owned(), decoded);
            return;
        }
        decoded.push(serde_json::from_value(value).map_err(LlmError::Json));
    }

    fn fail(&mut self, message: String, decoded: &mut Vec<Result<ChatStreamChunk, LlmError>>) {
        self.finished = true;
        self.buffer.clear();
        self.data.clear();
        decoded.push(Err(LlmError::Stream(message)));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::time::Instant;

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
        let event = format!("data: {CHUNK}\r\n\r\n");
        let (head, tail) = event.as_bytes().split_at(17);

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
                "api03-AAAAAAAAAAAAAAAAAAAAAAAA\"}}\n\n"
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
            .push(format!(": keep-alive\nevent: message\nid: 7\ndata: {CHUNK}\n\n").as_bytes());

        assert_eq!(decoded.len(), 1);
    }

    #[test]
    fn a_malformed_chunk_is_reported_and_the_stream_goes_on() {
        let mut decoder = EventDecoder::default();

        let decoded = decoder.push(format!("data: {{not json\n\ndata: {CHUNK}\n\n").as_bytes());

        assert!(matches!(decoded[0], Err(LlmError::Json(_))));
        assert_eq!(content(&decoded[1]), Some("hi"));
        assert!(!decoder.is_finished());
    }

    #[test]
    fn a_long_line_arriving_in_small_reads_is_scanned_once() {
        let mut decoder = EventDecoder::default();
        let padding = "x".repeat(8 * 1024 * 1024);
        let event = format!("data: {{\"choices\":[],\"model\":\"{padding}\"}}\n\n");

        let started = Instant::now();
        let decoded: Vec<_> = event
            .as_bytes()
            .chunks(16 * 1024)
            .flat_map(|chunk| decoder.push(chunk))
            .collect();
        let elapsed = started.elapsed();

        assert_eq!(decoded.len(), 1);
        assert!(decoded[0].is_ok(), "{:?}", decoded[0].as_ref().err());
        assert!(
            elapsed < Duration::from_secs(2),
            "an 8 MiB line took {elapsed:?}"
        );
    }

    #[test]
    fn the_data_lines_of_one_event_are_joined_with_newlines() {
        let mut decoder = EventDecoder::default();

        let decoded = decoder.push(
            concat!(
                "data: {\"choices\":\n",
                "data:[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n",
                "\n"
            )
            .as_bytes(),
        );

        assert_eq!(decoded.len(), 1, "{decoded:?}");
        assert_eq!(content(&decoded[0]), Some("hi"));
    }

    #[test]
    fn an_event_is_dispatched_at_the_blank_line_that_ends_it() {
        let mut decoder = EventDecoder::default();

        assert!(
            decoder
                .push(format!("data: {CHUNK}\n").as_bytes())
                .is_empty()
        );
        let decoded = decoder.push(b"\n");

        assert_eq!(decoded.len(), 1);
        assert_eq!(content(&decoded[0]), Some("hi"));
    }

    #[test]
    fn an_event_left_open_at_the_end_of_the_stream_is_dispatched() {
        let mut decoder = EventDecoder::default();

        assert!(
            decoder
                .push(format!("data: {CHUNK}\n").as_bytes())
                .is_empty()
        );
        let decoded = decoder.finish();

        assert_eq!(decoded.len(), 1);
        assert_eq!(content(&decoded[0]), Some("hi"));
    }

    #[test]
    fn a_body_without_a_single_event_is_a_failure_not_an_empty_answer() {
        for body in [
            &b"{\"id\":\"c\",\"choices\":[]}\n"[..],
            b": keep-alive\n\n",
            b"",
        ] {
            let mut decoder = EventDecoder::default();

            let mut decoded = decoder.push(body);
            decoded.extend(decoder.finish());

            assert_eq!(decoded.len(), 1, "{decoded:?}");
            assert!(
                matches!(&decoded[0], Err(LlmError::Stream(message)) if message.contains(&format!("after {} bytes", body.len()))),
                "{decoded:?}"
            );
        }
    }

    #[test]
    fn a_stream_that_only_says_done_is_not_a_failure() {
        let mut decoder = EventDecoder::default();

        assert!(decoder.push(b"data: [DONE]\n\n").is_empty());
        assert!(decoder.finish().is_empty());
    }

    #[test]
    fn an_event_that_grows_past_the_limit_is_refused() {
        let mut decoder = EventDecoder::with_limit(64);

        let decoded = decoder.push("data: 0123456789abcdef\n".repeat(4).as_bytes());

        assert_eq!(decoded.len(), 1);
        assert!(
            matches!(&decoded[0], Err(LlmError::Stream(message)) if message.contains("64")),
            "{decoded:?}"
        );
        assert!(decoder.is_finished());
    }
}
