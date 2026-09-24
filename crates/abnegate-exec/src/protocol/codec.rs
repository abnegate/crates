//! NDJSON (Newline-Delimited JSON) codec for protocol messages.
//!
//! Each message is a single JSON object followed by a newline character.

use std::marker::PhantomData;

use bytes::Buf;
use bytes::BufMut;
use bytes::BytesMut;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio_util::codec::Decoder;
use tokio_util::codec::Encoder;

use crate::error::ProtocolError;

/// Maximum line length to prevent memory exhaustion
const MAX_LINE_LENGTH: usize = 16 * 1024 * 1024;

const NEWLINE: u8 = b'\n';

/// NDJSON codec that serializes/deserializes JSON messages with newline delimiters.
///
/// Each message is encoded as a single JSON object followed by `\n`.
/// Decoding reads lines and parses them as JSON, skipping blank lines.
pub struct NdjsonCodec<T> {
    /// Maximum allowed line length
    max_length: usize,
    /// How much of the buffer is already known to hold no newline, so a line
    /// arriving in many reads is scanned once rather than once per read
    scanned: usize,
    message: PhantomData<T>,
}

impl<T> NdjsonCodec<T> {
    /// Create a new NDJSON codec with default max line length
    pub fn new() -> Self {
        Self::with_max_length(MAX_LINE_LENGTH)
    }

    /// Create a new NDJSON codec with custom max line length
    pub fn with_max_length(max_length: usize) -> Self {
        Self {
            max_length,
            scanned: 0,
            message: PhantomData,
        }
    }
}

impl<T> Default for NdjsonCodec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Clone for NdjsonCodec<T> {
    /// A clone decodes its own buffer, so it starts with nothing scanned.
    fn clone(&self) -> Self {
        Self::with_max_length(self.max_length)
    }
}

impl<T: DeserializeOwned> Decoder for NdjsonCodec<T> {
    type Item = T;
    type Error = ProtocolError;

    fn decode(&mut self, source: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        loop {
            self.scanned = self.scanned.min(source.len());
            let Some(offset) = source[self.scanned..]
                .iter()
                .position(|byte| *byte == NEWLINE)
            else {
                self.scanned = source.len();
                if source.len() > self.max_length {
                    return Err(ProtocolError::LineTooLong {
                        length: source.len(),
                        max: self.max_length,
                    });
                }
                return Ok(None);
            };

            let length = self.scanned + offset;
            self.scanned = 0;
            let line = source.split_to(length);
            source.advance(1);

            if length > self.max_length {
                return Err(ProtocolError::LineTooLong {
                    length,
                    max: self.max_length,
                });
            }
            if line.trim_ascii().is_empty() {
                continue;
            }

            return serde_json::from_slice(&line).map(Some).map_err(|cause| {
                ProtocolError::JsonParse {
                    length,
                    category: cause.classify(),
                    line: cause.line(),
                    column: cause.column(),
                }
            });
        }
    }
}

impl<T: Serialize> Encoder<T> for NdjsonCodec<T> {
    type Error = ProtocolError;

    fn encode(&mut self, item: T, destination: &mut BytesMut) -> Result<(), Self::Error> {
        let json = serde_json::to_string(&item).map_err(ProtocolError::JsonSerialize)?;

        destination.reserve(json.len() + 1);
        destination.put_slice(json.as_bytes());
        destination.put_u8(NEWLINE);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::protocol::ErrorCode;
    use crate::protocol::InboundMessage;
    use crate::protocol::LogLevel;
    use crate::protocol::OutboundMessage;

    use super::*;

    #[test]
    fn test_decode_single_message() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::from(
            r#"{"type":"Hello","protocol_version":"1.0","capabilities":[]}"#.as_bytes(),
        );
        buffer.extend_from_slice(b"\n");

        let result = codec.decode(&mut buffer).unwrap();
        assert!(result.is_some());

        match result.unwrap() {
            InboundMessage::Hello(hello) => {
                assert_eq!(hello.protocol_version, "1.0");
            }
            _ => panic!("Wrong message type"),
        }

        assert!(buffer.is_empty());
    }

    #[test]
    fn test_decode_partial_message() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::from(r#"{"type":"Hello","protocol_version":"1.0""#.as_bytes());

        let result = codec.decode(&mut buffer).unwrap();
        assert!(result.is_none());

        buffer.extend_from_slice(r#","capabilities":[]}"#.as_bytes());
        buffer.extend_from_slice(b"\n");

        let result = codec.decode(&mut buffer).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn test_decode_multiple_messages() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::from(
            r#"{"type":"Ping","id":"1"}
{"type":"Ping","id":"2"}
"#
            .as_bytes(),
        );

        let first = codec.decode(&mut buffer).unwrap().unwrap();
        let second = codec.decode(&mut buffer).unwrap().unwrap();

        match first {
            InboundMessage::Ping(ping) => assert_eq!(ping.id, "1"),
            _ => panic!("Wrong message type"),
        }

        match second {
            InboundMessage::Ping(ping) => assert_eq!(ping.id, "2"),
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_decode_many_messages_sequentially() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::new();

        for index in 0..100 {
            buffer.extend_from_slice(format!(r#"{{"type":"Ping","id":"{}"}}"#, index).as_bytes());
            buffer.extend_from_slice(b"\n");
        }

        for index in 0..100 {
            let message = codec.decode(&mut buffer).unwrap().unwrap();
            match message {
                InboundMessage::Ping(ping) => assert_eq!(ping.id, index.to_string()),
                _ => panic!("Wrong message type"),
            }
        }

        assert!(buffer.is_empty());
    }

    #[test]
    fn test_encode_message() {
        let mut codec: NdjsonCodec<OutboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::new();

        let message = OutboundMessage::Pong {
            id: "test".to_string(),
        };

        codec.encode(message, &mut buffer).unwrap();

        let text = String::from_utf8(buffer.to_vec()).unwrap();
        assert!(text.ends_with('\n'));
        assert!(text.contains(r#""type":"Pong""#));
        assert!(text.contains(r#""id":"test""#));
    }

    #[test]
    fn test_encode_multiple_messages() {
        let mut codec: NdjsonCodec<OutboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::new();

        codec
            .encode(
                OutboundMessage::Pong {
                    id: "1".to_string(),
                },
                &mut buffer,
            )
            .unwrap();
        codec
            .encode(
                OutboundMessage::Pong {
                    id: "2".to_string(),
                },
                &mut buffer,
            )
            .unwrap();
        codec
            .encode(
                OutboundMessage::Pong {
                    id: "3".to_string(),
                },
                &mut buffer,
            )
            .unwrap();

        let text = String::from_utf8(buffer.to_vec()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_encode_all_outbound_message_types() {
        let mut codec: NdjsonCodec<OutboundMessage> = NdjsonCodec::new();

        let messages = vec![
            OutboundMessage::hello_ack(),
            OutboundMessage::RunStarted {
                job_id: "j1".to_string(),
                pid: 123,
            },
            OutboundMessage::RunStdout {
                job_id: "j1".to_string(),
                data: "dGVzdA==".to_string(),
                sequence: 1,
            },
            OutboundMessage::RunStderr {
                job_id: "j1".to_string(),
                data: "ZXJy".to_string(),
                sequence: 1,
            },
            OutboundMessage::RunLog {
                job_id: "j1".to_string(),
                level: LogLevel::Info,
                message: "test".to_string(),
                details: None,
            },
            OutboundMessage::RunExit {
                job_id: "j1".to_string(),
                exit_code: Some(0),
                signal: None,
                duration_ms: 100,
            },
            OutboundMessage::RunError {
                job_id: "j1".to_string(),
                error_code: ErrorCode::Timeout,
                message: "timeout".to_string(),
            },
            OutboundMessage::Pong {
                id: "p1".to_string(),
            },
        ];

        for message in messages {
            let mut buffer = BytesMut::new();
            assert!(codec.encode(message, &mut buffer).is_ok());
            assert!(!buffer.is_empty());
            assert!(buffer.last() == Some(&b'\n'));
        }
    }

    #[test]
    fn test_decode_empty_line() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::from("\n".as_bytes());

        let result = codec.decode(&mut buffer).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_decode_multiple_empty_lines() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::from("\n\n\n".as_bytes());

        assert!(codec.decode(&mut buffer).unwrap().is_none());
        assert!(buffer.is_empty());
    }

    /// A framed reader takes `None` to mean "read more", so a blank line that
    /// answered `None` left the message after it waiting for bytes that might
    /// never come.
    #[test]
    fn an_empty_line_does_not_hold_back_the_message_after_it() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::from("\n\n{\"type\":\"Ping\",\"id\":\"1\"}\n".as_bytes());

        let message = codec.decode(&mut buffer).unwrap().unwrap();
        match message {
            InboundMessage::Ping(ping) => assert_eq!(ping.id, "1"),
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_decode_invalid_json() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::from("not valid json\n".as_bytes());

        let result = codec.decode(&mut buffer);
        assert!(result.is_err());

        match result.unwrap_err() {
            ProtocolError::JsonParse { length, .. } => {
                assert_eq!(length, "not valid json".len());
            }
            _ => panic!("Wrong error type"),
        }
    }

    #[test]
    fn a_rejected_line_never_reaches_the_error() {
        const SECRET: &str = "hunter2-master-key";
        for line in [
            format!(
                r#"{{"type":"RunStart","job_id":"j","workspace":"/tmp","command":"ls","env":"{SECRET}"}}"#
            ),
            format!(
                r#"{{"type":"RunStart","job_id":"j","workspace":"/tmp","command":"ls","env":{{"APP_MASTER_KEY":"{SECRET}"}},"timeout_ms":"soon"}}"#
            ),
        ] {
            let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
            let mut buffer = BytesMut::from(format!("{line}\n").as_bytes());

            let error = codec.decode(&mut buffer).unwrap_err();

            assert!(!error.to_string().contains(SECRET), "{error}");
            assert!(!format!("{error:?}").contains(SECRET), "{error:?}");
            assert!(
                std::error::Error::source(&error).is_none(),
                "serde's message repeats the value it rejected"
            );
            assert!(matches!(
                error,
                ProtocolError::JsonParse { length, .. } if length == line.len()
            ));
        }
    }

    #[test]
    fn test_decode_truncated_json() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::from("{\"type\":\"Ping\"\n".as_bytes());

        let result = codec.decode(&mut buffer);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_wrong_type() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::from("{\"type\":\"InvalidType\",\"foo\":\"bar\"}\n".as_bytes());

        let result = codec.decode(&mut buffer);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_missing_required_fields() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::from("{\"type\":\"RunStart\"}\n".as_bytes());

        let result = codec.decode(&mut buffer);
        assert!(result.is_err());
    }

    #[test]
    fn test_line_too_long() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::with_max_length(10);
        let mut buffer = BytesMut::from("this line is way too long\n".as_bytes());

        let result = codec.decode(&mut buffer);
        assert!(result.is_err());

        match result.unwrap_err() {
            ProtocolError::LineTooLong { length, max } => {
                assert!(length > max);
                assert_eq!(max, 10);
            }
            _ => panic!("Wrong error type"),
        }
    }

    #[test]
    fn test_line_at_max_length() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::with_max_length(100);
        let json = r#"{"type":"Ping","id":"test"}"#;
        assert!(json.len() < 100);

        let mut buffer = BytesMut::from(format!("{}\n", json).as_bytes());
        let result = codec.decode(&mut buffer);
        assert!(result.is_ok());
    }

    #[test]
    fn test_buffer_growing_without_newline() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::with_max_length(50);
        let mut buffer = BytesMut::new();

        buffer.extend_from_slice("a".repeat(60).as_bytes());

        let result = codec.decode(&mut buffer);
        assert!(result.is_err());

        match result.unwrap_err() {
            ProtocolError::LineTooLong { length, max } => {
                assert_eq!(length, 60);
                assert_eq!(max, 50);
            }
            _ => panic!("Wrong error type"),
        }
    }

    #[test]
    fn test_roundtrip() {
        let mut encoder: NdjsonCodec<OutboundMessage> = NdjsonCodec::new();
        let mut decoder: NdjsonCodec<OutboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::new();

        let original = OutboundMessage::RunExit {
            job_id: "test-job".to_string(),
            exit_code: Some(0),
            signal: None,
            duration_ms: 1234,
        };

        encoder.encode(original.clone(), &mut buffer).unwrap();
        let decoded = decoder.decode(&mut buffer).unwrap().unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn test_roundtrip_all_message_types() {
        let messages = vec![
            OutboundMessage::hello_ack(),
            OutboundMessage::RunStarted {
                job_id: "j1".to_string(),
                pid: 12345,
            },
            OutboundMessage::RunStdout {
                job_id: "j1".to_string(),
                data: "SGVsbG8gV29ybGQ=".to_string(),
                sequence: 42,
            },
            OutboundMessage::RunStderr {
                job_id: "j1".to_string(),
                data: "RXJyb3I=".to_string(),
                sequence: 1,
            },
            OutboundMessage::RunLog {
                job_id: "j1".to_string(),
                level: LogLevel::Warn,
                message: "Test warning".to_string(),
                details: Some(serde_json::json!({"key": "value"})),
            },
            OutboundMessage::RunExit {
                job_id: "j1".to_string(),
                exit_code: Some(1),
                signal: None,
                duration_ms: 5000,
            },
            OutboundMessage::RunExit {
                job_id: "j2".to_string(),
                exit_code: None,
                signal: Some(9),
                duration_ms: 100,
            },
            OutboundMessage::RunError {
                job_id: "j1".to_string(),
                error_code: ErrorCode::Cancelled,
                message: "Cancelled".to_string(),
            },
            OutboundMessage::Pong {
                id: "ping-123".to_string(),
            },
        ];

        for original in messages {
            let mut encoder: NdjsonCodec<OutboundMessage> = NdjsonCodec::new();
            let mut decoder: NdjsonCodec<OutboundMessage> = NdjsonCodec::new();
            let mut buffer = BytesMut::new();

            encoder.encode(original.clone(), &mut buffer).unwrap();
            let decoded = decoder.decode(&mut buffer).unwrap().unwrap();

            assert_eq!(original, decoded);
        }
    }

    #[test]
    fn test_decode_unicode_content() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::from(r#"{"type":"Ping","id":"测试🎉"}"#.as_bytes());
        buffer.extend_from_slice(b"\n");

        let result = codec.decode(&mut buffer).unwrap().unwrap();
        match result {
            InboundMessage::Ping(ping) => {
                assert_eq!(ping.id, "测试🎉");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_encode_unicode_content() {
        let mut codec: NdjsonCodec<OutboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::new();

        let message = OutboundMessage::Pong {
            id: "Привет мир 🌍".to_string(),
        };

        codec.encode(message, &mut buffer).unwrap();

        let text = String::from_utf8(buffer.to_vec()).unwrap();
        assert!(text.contains("Привет") || text.contains("\\u"));
    }

    #[test]
    fn a_partial_line_is_scanned_once() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::from(r#"{"type":"Ping","#.as_bytes());

        assert!(codec.decode(&mut buffer).unwrap().is_none());
        assert_eq!(codec.scanned, buffer.len());

        buffer.extend_from_slice(br#""id":"1"}"#);
        assert!(codec.decode(&mut buffer).unwrap().is_none());
        assert_eq!(codec.scanned, buffer.len());

        buffer.extend_from_slice(b"\n");
        assert!(codec.decode(&mut buffer).unwrap().is_some());
        assert_eq!(codec.scanned, 0);
    }

    #[test]
    fn a_buffer_emptied_between_reads_is_scanned_from_its_start() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::from("partial".as_bytes());
        assert!(codec.decode(&mut buffer).unwrap().is_none());

        let mut fresh = BytesMut::from("{\"type\":\"Ping\",\"id\":\"1\"}\n".as_bytes());

        assert!(codec.decode(&mut BytesMut::new()).unwrap().is_none());
        assert!(codec.decode(&mut fresh).unwrap().is_some());
    }

    #[test]
    fn a_clone_decodes_a_fresh_buffer_from_its_start() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        assert!(
            codec
                .decode(&mut BytesMut::from("partial".as_bytes()))
                .unwrap()
                .is_none()
        );
        let mut clone = codec.clone();
        let mut fresh = BytesMut::from(
            "{\"type\":\"Ping\",\"id\":\"1\"}\n{\"type\":\"Ping\",\"id\":\"2\"}\n".as_bytes(),
        );

        assert!(clone.decode(&mut fresh).unwrap().is_some());
        assert!(clone.decode(&mut fresh).unwrap().is_some());
    }

    #[test]
    fn a_line_of_only_whitespace_is_skipped() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let mut buffer = BytesMut::from(" \r\n\t\n{\"type\":\"Ping\",\"id\":\"1\"}\n".as_bytes());

        assert!(codec.decode(&mut buffer).unwrap().is_some());
    }

    #[test]
    fn test_codec_clone() {
        let first: NdjsonCodec<InboundMessage> = NdjsonCodec::with_max_length(1000);
        let second = first.clone();

        assert_eq!(first.max_length, second.max_length);
    }

    #[test]
    fn test_codec_default() {
        let first: NdjsonCodec<InboundMessage> = NdjsonCodec::default();
        let second: NdjsonCodec<InboundMessage> = NdjsonCodec::new();

        assert_eq!(first.max_length, second.max_length);
    }

    #[test]
    fn test_decode_large_message() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();

        let large_data = "x".repeat(100_000);
        let json = format!(
            r#"{{"type":"RunStdin","job_id":"j1","data":"{}"}}"#,
            large_data
        );
        let mut buffer = BytesMut::from(format!("{}\n", json).as_bytes());

        let result = codec.decode(&mut buffer);
        assert!(result.is_ok());

        match result.unwrap().unwrap() {
            InboundMessage::RunStdin(stdin) => {
                assert_eq!(stdin.data.len(), 100_000);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_incremental_decode() {
        let mut codec: NdjsonCodec<InboundMessage> = NdjsonCodec::new();
        let full_message = r#"{"type":"Ping","id":"test123"}"#;
        let mut buffer = BytesMut::new();

        for (index, byte) in full_message.bytes().enumerate() {
            buffer.extend_from_slice(&[byte]);

            if index < full_message.len() - 1 {
                assert!(codec.decode(&mut buffer).unwrap().is_none());
            }
        }

        buffer.extend_from_slice(b"\n");

        let result = codec.decode(&mut buffer).unwrap().unwrap();
        match result {
            InboundMessage::Ping(ping) => assert_eq!(ping.id, "test123"),
            _ => panic!("Wrong message type"),
        }
    }
}
